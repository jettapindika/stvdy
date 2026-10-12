//! The QRIS HTTP client: transport and the trait's shape, nothing else.
//!
//! Every gateway-specific name — the paths, the auth header, the request body, the
//! response fields — is in [`mapping`](super::mapping). This file is what turns those
//! into a `reqwest` call and a [`PaymentError`], so B1's arrival does not touch it.
//!
//! It is the only place in the product that talks to the gateway. A client never
//! does (spec § Payments), and a route never does directly: it goes through
//! [`PaymentGateway`](super::PaymentGateway), which is what makes the mock and the
//! real client interchangeable in every test.

use std::time::Duration;

use axum::http::HeaderMap;
use secrecy::{ExposeSecret, SecretString};

use stvdy_core::traits::payment::{
    GatewayStatus, Invoice, InvoiceRequest, PaymentError, PaymentProvider, WebhookEvent,
};

use crate::config::{QrisConfig, SignatureScheme};

use super::mapping;
use super::signature;
use super::{GatewayTransaction, PaymentGateway};

/// How long one gateway call may take.
///
/// The checkout route calls `create_invoice` inside its transaction (a pending row
/// with no QR is worse than no row), so this bound is also the longest a connection
/// can be held by a slow gateway. Ten seconds is generous for a JSON round trip and
/// short enough that a hung gateway answers `provider_error` rather than stalling.
const GATEWAY_TIMEOUT: Duration = Duration::from_secs(10);

/// The QRIS gateway, as configured.
pub struct QrisGateway {
    client: reqwest::Client,
    base_url: String,
    api_key: SecretString,
    callback_token: SecretString,
    signature_scheme: SignatureScheme,
    signature_header: String,
    fee_percent: f64,
    fee_fixed_idr: i64,
}

impl QrisGateway {
    /// Builds the client from its configuration.
    ///
    /// The scheme is already a closed enum by the time this runs — `config::qris_config`
    /// refuses an unknown value at boot (A292) — so there is no "unknown scheme" arm
    /// here to accidentally default.
    pub fn from_config(config: &QrisConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(GATEWAY_TIMEOUT)
            .build()
            .map_err(|error| anyhow::anyhow!("building the payment HTTP client: {error}"))?;

        Ok(Self {
            client,
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            callback_token: config.callback_token.clone(),
            signature_scheme: config.signature_scheme,
            signature_header: config.signature_header.clone(),
            fee_percent: config.fee_percent,
            fee_fixed_idr: config.fee_fixed_idr,
        })
    }

    /// The fee this gateway charges for `amount_idr`, from the configured rate.
    ///
    /// Only used when the gateway's own response does not carry a fee. The default
    /// rate is zero, which means "unknown, record nothing" rather than a guess (A304):
    /// a made-up rate would silently corrupt the margin report, and `0` here is
    /// recorded as `0` only because the operator configured no rate.
    pub fn configured_fee_idr(&self, amount_idr: i64) -> i64 {
        let percent = (amount_idr as f64) * self.fee_percent / 100.0;
        // Whole rupiah, truncated, the same integer convention `billing::price` uses.
        self.fee_fixed_idr + percent as i64
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// One authenticated request, with the mapping's auth header.
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let (header, value) = mapping::auth_header(self.api_key.expose_secret());
        self.client
            .request(method, self.url(path))
            .header(header, value)
    }
}

#[async_trait::async_trait]
impl PaymentProvider for QrisGateway {
    fn name(&self) -> &'static str {
        // The value `payments.provider` stores, so a row's provider names the thing
        // that wrote it.
        "qris_gateway"
    }

    async fn create_invoice(&self, request: InvoiceRequest) -> Result<Invoice, PaymentError> {
        let started = std::time::Instant::now();
        let response = self
            .request(reqwest::Method::POST, mapping::invoice_path())
            .json(&mapping::invoice_body(&request))
            .send()
            .await
            .map_err(|error| PaymentError::Transport(error.to_string()))?;

        let status = response.status();
        let elapsed_ms = started.elapsed().as_millis() as u64;
        tracing::info!(
            order_id = %request.order_id,
            http_status = status.as_u16(),
            elapsed_ms,
            "payment invoice created"
        );

        if !status.is_success() {
            return Err(PaymentError::Provider(format!(
                "the gateway answered {} for an invoice",
                status.as_u16()
            )));
        }

        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| PaymentError::Invalid(format!("the invoice is not JSON: {error}")))?;
        mapping::invoice_from_response(&value)
    }

    async fn get_status(&self, order_id: &str) -> Result<GatewayStatus, PaymentError> {
        let started = std::time::Instant::now();
        let response = self
            .request(reqwest::Method::GET, &mapping::status_path(order_id))
            .send()
            .await
            .map_err(|error| PaymentError::Transport(error.to_string()))?;

        let status = response.status();
        tracing::info!(
            order_id,
            http_status = status.as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "payment status read"
        );

        // A 404 is the gateway saying it does not know this order, which the poller
        // turns into `review` rather than a retry loop. Collapsing it into
        // `Transport` would make an order we never created look like an outage.
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(PaymentError::UnknownOrder);
        }
        if !status.is_success() {
            return Err(PaymentError::Provider(format!(
                "the gateway answered {} for a status",
                status.as_u16()
            )));
        }

        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| PaymentError::Invalid(format!("the status is not JSON: {error}")))?;
        mapping::status_from_response(&value)
    }

    fn parse_and_verify_webhook(
        &self,
        headers: &HeaderMap,
        raw_body: &[u8],
    ) -> Result<WebhookEvent, PaymentError> {
        // Signature first, parse second: a body that fails the signature is never
        // deserialised, so a forged payload cannot decide which failure is reported.
        signature::verify(
            self.signature_scheme,
            headers,
            raw_body,
            &self.callback_token,
            &self.signature_header,
        )?;

        let raw: serde_json::Value = serde_json::from_slice(raw_body).map_err(|error| {
            PaymentError::Invalid(format!("the callback body is not JSON: {error}"))
        })?;
        mapping::webhook_from_body(&raw)
    }

    async fn refund_note(&self, order_id: &str, amount_idr: i64) -> Result<(), PaymentError> {
        // Record-only for the MVP: the spec says refunds are processed outside the app
        // and recorded in `payments` (A308), so there is no gateway call to make. The
        // method exists on the trait so the day a gateway does expose a refund API, the
        // call site is already there — and it is deliberately not a silent success on a
        // money movement: the row is what an operator acts on.
        tracing::info!(order_id, amount_idr, "refund noted for manual processing");
        Ok(())
    }
}

#[async_trait::async_trait]
impl PaymentGateway for QrisGateway {
    /// The gateway's recent transactions, for the reconciliation sweep (A301).
    async fn recent_transactions(
        &self,
        since_hours: i64,
    ) -> Result<Vec<GatewayTransaction>, PaymentError> {
        let started = std::time::Instant::now();
        let response = self
            .request(reqwest::Method::GET, mapping::transactions_path())
            .query(&[(mapping::transactions_since_param(), since_hours.to_string())])
            .send()
            .await
            .map_err(|error| PaymentError::Transport(error.to_string()))?;

        let status = response.status();
        tracing::info!(
            since_hours,
            http_status = status.as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "payment transactions listed"
        );

        if !status.is_success() {
            return Err(PaymentError::Provider(format!(
                "the gateway answered {} for the transaction list",
                status.as_u16()
            )));
        }

        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| PaymentError::Invalid(format!("the list is not JSON: {error}")))?;
        mapping::transactions_from_response(&value)
    }
}
