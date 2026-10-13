//! The gopay-bridge HTTP client: transport and the trait's shape, nothing else.
//!
//! Every gateway-specific name — the paths, the request body, the response fields — is
//! in [`mapping`](super::mapping). This file turns those into a `reqwest` call and a
//! [`PaymentError`], so a bridge change is a diff there.
//!
//! It is the only place in the product that talks to the gateway. A client never does
//! (spec § Payments), and a route never does directly: it goes through
//! [`PaymentGateway`](super::PaymentGateway), which is what makes the double and the
//! real client interchangeable in every test.
//!
//! **There is no auth header and no signature here, because the bridge has neither.**
//! It is reached over the compose network as `http://gopay-bridge:8787` — it publishes
//! no host port, and its own session to GoPay is the bridge's private business. That
//! is a deployment property, not a defence: the bridge is unauthenticated by design,
//! so *where it is reachable from* is the control.

use std::time::Duration;

use stvdy_core::traits::payment::{
    GatewayStatus, Invoice, InvoiceRequest, PaymentError, PaymentProvider,
};

use crate::config::QrisConfig;

use super::mapping;
use super::{GatewayTransaction, PaymentGateway};

/// How long one gateway call may take.
///
/// The checkout route calls `create_invoice` inside its transaction (a pending row with
/// no QR is worse than no row), so this bound is also the longest a connection can be
/// held by a slow gateway. Ten seconds is generous for a local round trip and short
/// enough that a hung bridge answers `provider_error` rather than stalling.
const GATEWAY_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the reconciliation sweep may wait for the transaction feed.
///
/// Longer than [`GATEWAY_TIMEOUT`], and for a different reason: the bridge's feed route
/// reaches GoPay and paginates, while `create` and the status read are answered from the
/// bridge's own state. The sweep runs hourly in the background, so waiting here costs
/// nothing a user can feel — but it is still bounded, because a job that can hang
/// forever is a job that never reports.
const FEED_TIMEOUT: Duration = Duration::from_secs(30);

/// The QRIS bridge, as configured.
pub struct QrisGateway {
    client: reqwest::Client,
    base_url: String,
    merchant_id: String,
    invoice_ttl_minutes: i64,
}

impl QrisGateway {
    /// Builds the client from its configuration.
    pub fn from_config(config: &QrisConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(GATEWAY_TIMEOUT)
            .build()
            .map_err(|error| anyhow::anyhow!("building the payment HTTP client: {error}"))?;

        Ok(Self {
            client,
            base_url: config.base_url.clone(),
            merchant_id: config.merchant_id.clone(),
            invoice_ttl_minutes: config.invoice_ttl_minutes,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client.request(method, self.url(path))
    }

    /// Reads a JSON body, reporting a non-JSON answer as [`PaymentError::Invalid`]
    /// rather than as a transport failure: a bridge that answered 200 with something
    /// else is a contract break, not an outage.
    async fn json_body(response: reqwest::Response) -> Result<serde_json::Value, PaymentError> {
        response
            .json()
            .await
            .map_err(|error| PaymentError::Invalid(format!("the answer is not JSON: {error}")))
    }
}

#[async_trait::async_trait]
impl PaymentProvider for QrisGateway {
    fn name(&self) -> &'static str {
        // The value `payments.provider` stores, so a row's provider names the thing that
        // wrote it. Unchanged by B1's resolution: it is our name for this gateway, and
        // every row already written carries it.
        "qris_gateway"
    }

    async fn create_invoice(&self, request: InvoiceRequest) -> Result<Invoice, PaymentError> {
        let started = std::time::Instant::now();
        let response = self
            .request(reqwest::Method::POST, mapping::invoice_path())
            .json(&mapping::invoice_body(&request, self.invoice_ttl_minutes))
            .send()
            .await
            .map_err(|error| PaymentError::Transport(error.to_string()))?;

        let status = response.status();
        tracing::info!(
            order_id = %request.order_id,
            amount_idr = request.amount_idr,
            http_status = status.as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "payment invoice created"
        );

        if !status.is_success() {
            return Err(PaymentError::Provider(format!(
                "the gateway answered {} for an invoice",
                status.as_u16()
            )));
        }

        mapping::invoice_from_response(&Self::json_body(response).await?)
    }

    async fn get_status(&self, provider_payment_id: &str) -> Result<GatewayStatus, PaymentError> {
        let started = std::time::Instant::now();
        let response = self
            .request(
                reqwest::Method::GET,
                &mapping::status_path(provider_payment_id),
            )
            .send()
            .await
            .map_err(|error| PaymentError::Transport(error.to_string()))?;

        let status = response.status();
        tracing::info!(
            provider_payment_id,
            http_status = status.as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "payment status read"
        );

        // A 404 is the bridge saying it does not know this payment — which after a
        // bridge restart with a fresh state file is the honest answer, and one the
        // poller turns into `review` rather than a retry loop. Collapsing it into
        // `Transport` would make a payment we never minted look like an outage.
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(PaymentError::UnknownOrder);
        }
        if !status.is_success() {
            return Err(PaymentError::Provider(format!(
                "the gateway answered {} for a status",
                status.as_u16()
            )));
        }

        mapping::status_from_response(&Self::json_body(response).await?)
    }

    async fn refund_note(&self, order_id: &str, amount_idr: i64) -> Result<(), PaymentError> {
        // Record-only for the MVP: the spec says refunds are processed outside the app
        // and recorded in `payments` (A308), and the bridge has no refund route to call
        // — a QRIS refund is a GoPay merchant-dashboard action. The method exists on the
        // trait so the day a gateway does expose one, the call site is already there —
        // and it is deliberately not a silent success on a money movement: the row is
        // what an operator acts on.
        tracing::info!(order_id, amount_idr, "refund noted for manual processing");
        Ok(())
    }
}

#[async_trait::async_trait]
impl PaymentGateway for QrisGateway {
    /// The merchant's settled transactions, for the reconciliation sweep (A301).
    async fn recent_transactions(
        &self,
        since_hours: i64,
    ) -> Result<Vec<GatewayTransaction>, PaymentError> {
        let started = std::time::Instant::now();
        // Hours to minutes: the bridge bounds its feed query in minutes and clamps it to
        // a week, so a longer `QRIS_RECONCILE_HOURS` is answered with the widest window
        // it will serve rather than an error.
        let minutes = since_hours.saturating_mul(60);
        let response = self
            .request(reqwest::Method::GET, mapping::transactions_path())
            .query(&[
                (mapping::transactions_merchant_param(), &self.merchant_id),
                (mapping::transactions_minutes_param(), &minutes.to_string()),
            ])
            .timeout(FEED_TIMEOUT)
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

        mapping::transactions_from_response(&Self::json_body(response).await?)
    }
}
