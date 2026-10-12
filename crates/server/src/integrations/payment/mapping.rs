//! **The one file B1 changes.**
//!
//! The gateway has no identity, no docs and no response shapes (B1, unresolved), so
//! the honest design is an internal contract *we* define and *we* test, with every
//! gateway-specific name isolated here (A291). Nothing in this file is a guess about
//! a real provider: the names below are the internal contract's, and the comment on
//! each says so. When the docs arrive, the diff is this file, one arm of
//! [`signature`](super::signature) if the scheme is new, and the env values — no
//! behavioural test changes, because every behavioural test in this phase runs
//! against the trait's mock.
//!
//! The tests at the bottom are the contract: they assert the body we send and the
//! responses we accept, so a mapping change is a test change and not a silent one.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use stvdy_core::traits::payment::{
    GatewayStatus, Invoice, InvoiceRequest, PaymentError, WebhookEvent,
};

use super::GatewayTransaction;

/// The request path for creating an invoice.
///
/// **Internal contract.** The real gateway's path is unknown, and a path invented in
/// code would be a silent production failure on the day B1 resolves.
pub fn invoice_path() -> &'static str {
    "/invoices"
}

/// The request path for reading an order's status.
pub fn status_path(order_id: &str) -> String {
    format!("/invoices/{order_id}")
}

/// The path the reconciliation sweep lists recent transactions from.
pub fn transactions_path() -> &'static str {
    "/transactions"
}

/// The auth header we send, as `(name, value)`.
///
/// **Internal contract.** Bearer over the API key is the shape the plan pins; the
/// gateway's own auth scheme is one of the things B1 names.
pub fn auth_header(api_key: &str) -> (&'static str, String) {
    ("Authorization", format!("Bearer {api_key}"))
}

/// The query parameter the transactions list is bounded by, in hours.
pub fn transactions_since_param() -> &'static str {
    "since_hours"
}

/// The body we POST to [`invoice_path`].
///
/// The keys are the internal contract's, deliberately identical to
/// [`InvoiceRequest`]'s field names: a translation here would be a guess. What this
/// function exists for is that there is exactly one place to translate *from*.
pub fn invoice_body(request: &InvoiceRequest) -> Value {
    json!({
        "order_id": request.order_id,
        "amount_idr": request.amount_idr,
        "description": request.description,
        "expires_at": request.expires_at.to_rfc3339(),
    })
}

/// The invoice the gateway answers with.
///
/// Reads the internal contract's field names, and reports a missing one as
/// [`PaymentError::Invalid`] rather than defaulting: an invoice with no `qr_string` is
/// an invoice a student cannot pay, and returning an empty string would make the
/// checkout screen render a QR that encodes nothing.
pub fn invoice_from_response(value: &Value) -> Result<Invoice, PaymentError> {
    let order_id = string_field(value, "order_id")?;
    let qr_string = string_field(value, "qr_string")?;
    let expires_at = instant_field(value, "expires_at")?;
    let provider_payment_id = optional_string(value, "provider_payment_id");

    Ok(Invoice {
        order_id,
        qr_string,
        expires_at,
        provider_payment_id,
    })
}

/// The status of an order, as the poller reads it (A301).
pub fn status_from_response(value: &Value) -> Result<GatewayStatus, PaymentError> {
    let status = string_field(value, "status")?;
    match status.as_str() {
        "pending" => Ok(GatewayStatus::Pending),
        "paid" => Ok(GatewayStatus::Paid {
            paid_at: instant_field(value, "paid_at")?,
            amount_idr: integer_field(value, "amount_idr")?,
            provider_payment_id: optional_string(value, "provider_payment_id"),
            fee_idr: optional_integer(value, "fee_idr"),
        }),
        "expired" => Ok(GatewayStatus::Expired),
        "failed" => Ok(GatewayStatus::Failed),
        "refunded" => Ok(GatewayStatus::Refunded),
        other => Err(PaymentError::Invalid(format!(
            "the gateway reported an unknown status {other:?}"
        ))),
    }
}

/// The fields a callback carries, as [`WebhookEvent`].
///
/// Called **after** the signature verifies, and it is the only reader of a callback's
/// field names. `currency` is required rather than defaulted to `IDR`: a payload that
/// does not say what currency it is in cannot be compared against a rupiah amount
/// (A294), and defaulting would turn a missing field into a match.
pub fn webhook_from_body(raw: &Value) -> Result<WebhookEvent, PaymentError> {
    let event_id = string_field(raw, "event_id")?;
    let kind = match string_field(raw, "kind")?.as_str() {
        "paid" => stvdy_core::traits::payment::WebhookKind::Paid,
        "expired" => stvdy_core::traits::payment::WebhookKind::Expired,
        "failed" => stvdy_core::traits::payment::WebhookKind::Failed,
        "refunded" => stvdy_core::traits::payment::WebhookKind::Refunded,
        other => {
            return Err(PaymentError::Invalid(format!(
                "the callback names an unknown kind {other:?}"
            )));
        }
    };

    Ok(WebhookEvent {
        event_id,
        kind,
        order_id: string_field(raw, "order_id")?,
        amount_idr: integer_field(raw, "amount_idr")?,
        currency: string_field(raw, "currency")?,
        paid_at: optional_instant(raw, "paid_at"),
        provider_payment_id: optional_string(raw, "provider_payment_id"),
        fee_idr: optional_integer(raw, "fee_idr"),
        raw: raw.clone(),
    })
}

/// The gateway's recent transactions, for the reconciliation sweep (A301).
///
/// The list is read from a `transactions` array, and each entry carries the same
/// `status` vocabulary `status_from_response` reads: one vocabulary for an order's
/// state, whether it arrives from a status query or from the list.
pub fn transactions_from_response(value: &Value) -> Result<Vec<GatewayTransaction>, PaymentError> {
    let entries = value
        .get("transactions")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            PaymentError::Invalid("the gateway response has no transactions list".to_string())
        })?;

    entries
        .iter()
        .map(|entry| {
            Ok(GatewayTransaction {
                order_id: string_field(entry, "order_id")?,
                status: status_from_response(entry)?,
                amount_idr: integer_field(entry, "amount_idr")?,
            })
        })
        .collect()
}

/// The gateway's own fee for a paid order, where the response reports one.
///
/// `None` when the field is absent **and** when the gateway is not configured with a
/// rate: the margin report must be able to say "unknown" rather than record a
/// plausible-looking number (A304). When the response does not carry a fee, the caller
/// falls back to `QRIS_FEE_PERCENT` / `QRIS_FEE_FIXED_IDR`.
pub fn fee_from_response(value: &Value) -> Option<i64> {
    optional_integer(value, "fee_idr")
}

fn string_field(value: &Value, key: &str) -> Result<String, PaymentError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| PaymentError::Invalid(format!("the gateway response has no {key:?}")))
}

fn integer_field(value: &Value, key: &str) -> Result<i64, PaymentError> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| PaymentError::Invalid(format!("the gateway response has no {key:?}")))
}

fn instant_field(value: &Value, key: &str) -> Result<DateTime<Utc>, PaymentError> {
    optional_instant(value, key)
        .ok_or_else(|| PaymentError::Invalid(format!("the gateway response has no {key:?}")))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn optional_integer(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

/// RFC 3339, which is what `timestamptz` serialises as everywhere else in this tree.
fn optional_instant(value: &Value, key: &str) -> Option<DateTime<Utc>> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|instant| instant.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> InvoiceRequest {
        InvoiceRequest {
            order_id: "ord_1".to_string(),
            amount_idr: 5_000,
            description: "Stvdy Pro (monthly)".to_string(),
            expires_at: DateTime::parse_from_rfc3339("2026-10-12T09:15:00Z")
                .expect("a fixed instant")
                .with_timezone(&Utc),
        }
    }

    /// The request we send is the internal contract, and this is the assertion that
    /// makes a mapping change visible rather than silent.
    #[test]
    fn the_invoice_body_carries_the_order_id_amount_and_expiry() {
        let body = invoice_body(&request());

        assert_eq!(body["order_id"], "ord_1");
        assert_eq!(body["amount_idr"], 5_000);
        assert_eq!(body["description"], "Stvdy Pro (monthly)");
        assert_eq!(body["expires_at"], "2026-10-12T09:15:00+00:00");
        // The amount is an integer, not a float or a string: a gateway that reads
        // `"5000"` or `5000.0` is a gateway whose amount check means nothing.
        assert!(body["amount_idr"].is_i64());
    }

    #[test]
    fn the_invoice_response_is_read_field_by_field() {
        let invoice = invoice_from_response(&json!({
            "order_id": "ord_1",
            "qr_string": "00020101021226...",
            "expires_at": "2026-10-12T09:15:00Z",
            "provider_payment_id": "gw_1",
        }))
        .expect("a complete response parses");

        assert_eq!(invoice.order_id, "ord_1");
        assert_eq!(invoice.qr_string, "00020101021226...");
        assert_eq!(
            invoice.expires_at,
            request().expires_at,
            "the expiry the client counts down must be the gateway's own instant"
        );
        assert_eq!(invoice.provider_payment_id.as_deref(), Some("gw_1"));

        // A response missing the QR string is a hard failure: an invoice a student
        // cannot pay must not become an empty QR.
        assert!(matches!(
            invoice_from_response(
                &json!({ "order_id": "ord_1", "expires_at": "2026-10-12T09:15:00Z" })
            ),
            Err(PaymentError::Invalid(_))
        ));
        // A non-instant expiry is refused too, rather than silently dropped.
        assert!(matches!(
            invoice_from_response(&json!({
                "order_id": "ord_1",
                "qr_string": "x",
                "expires_at": "in fifteen minutes",
            })),
            Err(PaymentError::Invalid(_))
        ));
    }

    #[test]
    fn every_status_the_poller_reads_parses_and_an_unknown_one_does_not() {
        assert_eq!(
            status_from_response(&json!({ "status": "pending" })).expect("pending"),
            GatewayStatus::Pending
        );
        assert_eq!(
            status_from_response(&json!({ "status": "expired" })).expect("expired"),
            GatewayStatus::Expired
        );
        assert_eq!(
            status_from_response(&json!({ "status": "failed" })).expect("failed"),
            GatewayStatus::Failed
        );
        assert_eq!(
            status_from_response(&json!({ "status": "refunded" })).expect("refunded"),
            GatewayStatus::Refunded
        );

        let paid = status_from_response(&json!({
            "status": "paid",
            "paid_at": "2026-10-12T09:10:00Z",
            "amount_idr": 5_000,
            "provider_payment_id": "gw_1",
            "fee_idr": 75,
        }))
        .expect("paid");
        match paid {
            GatewayStatus::Paid {
                paid_at,
                amount_idr,
                provider_payment_id,
                fee_idr,
            } => {
                assert_eq!(paid_at.to_rfc3339(), "2026-10-12T09:10:00+00:00");
                assert_eq!(amount_idr, 5_000);
                assert_eq!(provider_payment_id.as_deref(), Some("gw_1"));
                assert_eq!(fee_idr, Some(75));
            }
            other => panic!("expected a paid status, got {other:?}"),
        }

        // A fee the gateway does not report is `None`, not zero: the two mean
        // different things to the margin report (A304).
        match status_from_response(&json!({
            "status": "paid",
            "paid_at": "2026-10-12T09:10:00Z",
            "amount_idr": 5_000,
        }))
        .expect("paid without a fee")
        {
            GatewayStatus::Paid { fee_idr, .. } => assert_eq!(fee_idr, None),
            other => panic!("expected a paid status, got {other:?}"),
        }

        // A `paid` status with no amount is a failure, not a zero: an amount we
        // invented would match the payment it is supposed to be checked against.
        assert!(matches!(
            status_from_response(&json!({ "status": "paid", "paid_at": "2026-10-12T09:10:00Z" })),
            Err(PaymentError::Invalid(_))
        ));

        assert!(matches!(
            status_from_response(&json!({ "status": "settled" })),
            Err(PaymentError::Invalid(_))
        ));
    }

    #[test]
    fn the_callback_body_is_read_into_the_event_the_money_path_uses() {
        let raw = json!({
            "event_id": "evt_1",
            "kind": "paid",
            "order_id": "ord_1",
            "amount_idr": 5_000,
            "currency": "IDR",
            "paid_at": "2026-10-12T09:10:00Z",
            "provider_payment_id": "gw_1",
            "fee_idr": 75,
            "anything_else": "kept in raw",
        });
        let event = webhook_from_body(&raw).expect("a complete callback parses");

        assert_eq!(event.event_id, "evt_1");
        assert_eq!(event.kind, stvdy_core::traits::payment::WebhookKind::Paid);
        assert_eq!(event.order_id, "ord_1");
        assert_eq!(event.amount_idr, 5_000);
        assert_eq!(event.currency, "IDR");
        assert_eq!(event.fee_idr, Some(75));
        assert_eq!(
            event.raw, raw,
            "the raw payload is what a human reads when the amount does not match"
        );

        // A currency that is not stated is a failure, not an assumed `IDR`: A294's
        // amount check is meaningless if we invent the unit.
        assert!(matches!(
            webhook_from_body(&json!({
                "event_id": "evt_1",
                "kind": "paid",
                "order_id": "ord_1",
                "amount_idr": 5_000,
            })),
            Err(PaymentError::Invalid(_))
        ));

        // An unknown kind is refused rather than defaulted to `Paid`.
        assert!(matches!(
            webhook_from_body(&json!({
                "event_id": "evt_1",
                "kind": "charged",
                "order_id": "ord_1",
                "amount_idr": 5_000,
                "currency": "IDR",
            })),
            Err(PaymentError::Invalid(_))
        ));

        // The non-paid kinds are read too, because the webhook must not treat an
        // expired or refunded callback as a paid one.
        for (raw_kind, expected) in [
            ("expired", stvdy_core::traits::payment::WebhookKind::Expired),
            ("failed", stvdy_core::traits::payment::WebhookKind::Failed),
            (
                "refunded",
                stvdy_core::traits::payment::WebhookKind::Refunded,
            ),
        ] {
            let event = webhook_from_body(&json!({
                "event_id": "evt_2",
                "kind": raw_kind,
                "order_id": "ord_1",
                "amount_idr": 5_000,
                "currency": "IDR",
            }))
            .expect("a known kind parses");
            assert_eq!(event.kind, expected);
        }
    }

    #[test]
    fn the_paths_and_the_auth_header_are_the_contract() {
        assert_eq!(invoice_path(), "/invoices");
        assert_eq!(status_path("ord_1"), "/invoices/ord_1");
        assert_eq!(transactions_path(), "/transactions");
        assert_eq!(transactions_since_param(), "since_hours");
        assert_eq!(
            auth_header("key_1"),
            ("Authorization", "Bearer key_1".to_string())
        );
    }

    #[test]
    fn the_fee_is_read_only_where_the_gateway_reports_one() {
        assert_eq!(fee_from_response(&json!({ "fee_idr": 75 })), Some(75));
        assert_eq!(fee_from_response(&json!({})), None);
    }

    #[test]
    fn the_transaction_list_is_read_for_the_reconciliation_sweep() {
        let list = transactions_from_response(&json!({
            "transactions": [
                {
                    "order_id": "ord_1",
                    "status": "paid",
                    "amount_idr": 5_000,
                    "paid_at": "2026-10-12T09:10:00Z",
                },
                { "order_id": "ord_2", "status": "pending", "amount_idr": 10_000 },
            ]
        }))
        .expect("a list parses");

        assert_eq!(list.len(), 2);
        assert_eq!(list[0].order_id, "ord_1");
        assert_eq!(list[0].amount_idr, 5_000);
        assert!(matches!(list[0].status, GatewayStatus::Paid { .. }));
        assert!(matches!(list[1].status, GatewayStatus::Pending));

        // A missing list is a failure, not an empty one: an empty list read from a
        // malformed response would make the sweep report every payment as unknown to
        // the gateway, which is a false alarm a human then has to chase.
        assert!(matches!(
            transactions_from_response(&json!({ "items": [] })),
            Err(PaymentError::Invalid(_))
        ));
        // An entry missing its order id is refused rather than skipped.
        assert!(matches!(
            transactions_from_response(&json!({
                "transactions": [{ "status": "pending", "amount_idr": 1 }]
            })),
            Err(PaymentError::Invalid(_))
        ));
    }
}
