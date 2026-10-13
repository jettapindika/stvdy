//! The gopay-bridge wire mapping — the one file the gateway's identity lives in.
//!
//! B1 is resolved. The gateway is a self-hosted QRIS bridge (`../paymentgateway` in
//! the development tree; its README is the API document) that wraps GoPay merchant
//! infrastructure: it derives a **dynamic** QRIS from the merchant's static payload,
//! watches the merchant transaction feed, and answers questions over a local HTTP
//! surface. Every path, request key and response field name the product depends on is
//! named here, so a bridge change is a diff in this file and a test change rather than
//! a silent production failure.
//!
//! Three properties of this gateway shape the rest of the money path, and each is
//! visible in the functions below:
//!
//!  * **It assigns the amount.** `uniqueAmount` is the amount we ask for plus an
//!    offset in `[1, 999]`, which is how the bridge tells concurrent orders apart and
//!    matches a settlement to one of them **by amount alone**. So the invoice carries
//!    a payable figure that is not the price, and every later check is against that
//!    figure.
//!  * **It mints its own id.** Our `order_id` travels as `reference` and is never a
//!    lookup key: every status question is asked with the bridge's `id`.
//!  * **It never pushes.** There is no callback to verify and no callback body to
//!    parse, which is why this file has no `webhook_from_body` and why the poller is
//!    the only reader of the status fields.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use stvdy_core::traits::payment::{GatewayStatus, Invoice, InvoiceRequest, PaymentError};

use super::GatewayTransaction;

/// The request path that mints an invoice.
pub fn invoice_path() -> &'static str {
    "/payments"
}

/// The request path that reads one payment's state.
///
/// Keyed by the id **the bridge minted**, never by our order id: the bridge's own
/// route is `/payment/{id}`, and asking with our reference would be a request for a
/// payment that does not exist under that name.
pub fn status_path(provider_payment_id: &str) -> String {
    format!("/payment/{provider_payment_id}")
}

/// The path the reconciliation sweep lists settled transactions from.
pub fn transactions_path() -> &'static str {
    "/transactions"
}

/// The query parameter the transactions list is bounded by, in minutes.
///
/// Minutes, not the `since_hours` the internal contract used: the bridge's handler
/// takes `minutes` and clamps it to a week.
pub fn transactions_minutes_param() -> &'static str {
    "minutes"
}

/// The query parameter naming the merchant whose feed is read.
pub fn transactions_merchant_param() -> &'static str {
    "merchant_id"
}

/// The body we POST to [`invoice_path`].
///
/// `minutes` rather than our `expires_at`: the bridge takes a *lifetime* and its own
/// clock computes the deadline. That is why the invoice's `expiresAt` is what we store
/// (A6) and what the checkout screen counts down against, rather than the instant we
/// computed before the call.
///
/// `description` is deliberately absent: the bridge has no field for it, and a key the
/// gateway ignores is a field nobody reads.
pub fn invoice_body(request: &InvoiceRequest, minutes: i64) -> Value {
    json!({
        "amount": request.amount_idr,
        "reference": request.order_id,
        "minutes": minutes,
    })
}

/// The invoice the bridge answers with.
///
/// `qrString` and `expiresAt` are required: an invoice with no QR is an invoice a
/// student cannot pay, and an invoice with no deadline is a countdown the client would
/// have to invent.
///
/// `uniqueAmount` is required too, and that is the load-bearing one. It is the figure
/// the buyer transfers and the figure the settlement is checked against; defaulting it
/// to the amount we asked for would make every real payment mismatch its invoice and
/// land in `review`, which is exactly the failure this field exists to prevent.
pub fn invoice_from_response(value: &Value) -> Result<Invoice, PaymentError> {
    Ok(Invoice {
        order_id: string_field(value, "reference")?,
        // Non-empty, not merely present: the checkout screen renders this string verbatim,
        // so an empty payload is a blank code the student scans and pays nothing with.
        qr_string: non_empty_field(value, "qrString")?,
        expires_at: instant_field(value, "expiresAt")?,
        provider_payment_id: optional_string(value, "id"),
        payable_amount_idr: Some(integer_field(value, "uniqueAmount")?),
    })
}

/// The bridge's view of one payment, as the poller reads it.
///
/// The vocabulary is the bridge's (`pending`, `paid`, `expired`, `cancelled`) mapped
/// onto the contract's. The two dead states stay apart: `cancelled` is a person or the
/// buyer stopping the invoice and `expired` is the clock running out, and the row
/// records which.
///
/// A `paid` answer reads `uniqueAmount` — what actually landed — and requires the
/// `transaction` the bridge attaches in the same transition that marks the payment
/// paid. Its `time` is the settlement instant A295's late-payment rule turns on, and
/// substituting our own clock for a missing one would silently decide that rule. The
/// bridge reports no fee, so `fee_idr` is `None` and the margin report falls back to
/// the configured rate, or records nothing (A304).
pub fn status_from_response(value: &Value) -> Result<GatewayStatus, PaymentError> {
    let status = string_field(value, "status")?;
    match status.as_str() {
        "pending" => Ok(GatewayStatus::Pending),
        "paid" => {
            let transaction = value.get("transaction").ok_or_else(|| {
                PaymentError::Invalid(
                    "the gateway reports a payment as paid with no transaction".to_string(),
                )
            })?;
            Ok(GatewayStatus::Paid {
                paid_at: instant_field(transaction, "time")?,
                amount_idr: integer_field(value, "uniqueAmount")?,
                provider_payment_id: optional_string(value, "id"),
                fee_idr: optional_integer(value, "fee_idr"),
            })
        }
        "expired" => Ok(GatewayStatus::Expired),
        "cancelled" => Ok(GatewayStatus::Cancelled),
        other => Err(PaymentError::Invalid(format!(
            "the gateway reported an unknown payment status {other:?}"
        ))),
    }
}

/// The settled transactions from the merchant feed, for the reconciliation sweep
/// (A301).
///
/// The bridge answers with a **bare array**, and every entry in it is money that
/// arrived: the feed is queried with `statuses=settlement,capture`, so an unsettled
/// transaction is not in this list at all and there is no status to filter here.
///
/// **`amount` is the join key, not `orderId`.** The feed carries the *provider's*
/// order id, which is not ours — the bridge never sends our `reference` back in the
/// feed — so the only field that identifies one of our payments is the amount that
/// landed. That is the same fact the bridge matches its own settlements by.
pub fn transactions_from_response(value: &Value) -> Result<Vec<GatewayTransaction>, PaymentError> {
    let entries = value.as_array().ok_or_else(|| {
        PaymentError::Invalid("the gateway's transaction list is not an array".to_string())
    })?;

    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        out.push(GatewayTransaction {
            transaction_id: string_field(entry, "id")?,
            amount_idr: integer_field(entry, "amount")?,
            paid_at: instant_field(entry, "time")?,
        });
    }
    Ok(out)
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

/// A string field that must also carry something.
///
/// Used for the QR payload alone, and the distinction is the point: a missing field and an
/// empty one are the same failure to a student — a code that pays nothing — and `string_field`
/// would pass `""` through.
fn non_empty_field(value: &Value, key: &str) -> Result<String, PaymentError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|raw| !raw.is_empty())
        .map(str::to_string)
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

/// RFC 3339, which is what the bridge serialises its instants as (`time.RFC3339`).
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

    /// The request we send, asserted field by field. A bridge that renames a key or
    /// starts requiring another one turns this red rather than answering 400 in
    /// production.
    #[test]
    fn the_invoice_request_is_the_bridges_shape() {
        let request = InvoiceRequest {
            order_id: "ord_1".to_string(),
            amount_idr: 10_000,
            description: "Stvdy pro (monthly)".to_string(),
            expires_at: Utc::now(),
        };

        let body = invoice_body(&request, 15);
        assert_eq!(
            body,
            json!({ "amount": 10_000, "reference": "ord_1", "minutes": 15 })
        );
    }

    /// The happy path: a full `POST /payments` answer, as the bridge writes it.
    #[test]
    fn a_created_invoice_parses_with_its_payable_amount() {
        let raw = json!({
            "id": "gw-7f2a",
            "status": "pending",
            "amount": 10_000,
            "uniqueAmount": 10_137,
            "qrString": "00020101021226...6304ABCD",
            "reference": "ord_1",
            "createdAt": "2026-10-13T03:00:00Z",
            "expiresAt": "2026-10-13T03:15:00Z",
            "secondsLeft": 900,
        });

        let invoice = invoice_from_response(&raw).unwrap();
        assert_eq!(invoice.order_id, "ord_1");
        assert_eq!(invoice.qr_string, "00020101021226...6304ABCD");
        assert_eq!(
            invoice.provider_payment_id.as_deref(),
            Some("gw-7f2a"),
            "the gateway's id is the handle every later question is asked with"
        );
        assert_eq!(
            invoice.payable_amount_idr,
            Some(10_137),
            "the offset is the gateway's, and it is the figure the buyer transfers"
        );
        assert_eq!(
            invoice.expires_at,
            DateTime::parse_from_rfc3339("2026-10-13T03:15:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    /// An invoice with no payable amount is refused rather than defaulted: a fallback
    /// to the requested amount would make every real payment mismatch its invoice.
    #[test]
    fn an_invoice_without_a_payable_amount_is_refused() {
        let raw = json!({
            "id": "gw-1",
            "qrString": "QR",
            "reference": "ord_1",
            "expiresAt": "2026-10-13T03:15:00Z",
        });

        assert!(matches!(
            invoice_from_response(&raw),
            Err(PaymentError::Invalid(_))
        ));
    }

    /// Every status the bridge can report, and the two failures: an unknown word and a
    /// `paid` answer with no transaction behind it.
    #[test]
    fn the_bridges_status_vocabulary_maps_onto_the_contracts() {
        assert_eq!(
            status_from_response(&json!({ "status": "pending" })).unwrap(),
            GatewayStatus::Pending
        );
        assert_eq!(
            status_from_response(&json!({ "status": "expired" })).unwrap(),
            GatewayStatus::Expired
        );
        assert_eq!(
            status_from_response(&json!({ "status": "cancelled" })).unwrap(),
            GatewayStatus::Cancelled
        );

        let paid = status_from_response(&json!({
            "id": "gw-7f2a",
            "status": "paid",
            "amount": 10_000,
            "uniqueAmount": 10_137,
            "transaction": { "id": "tx-1", "amount": 10_137, "time": "2026-10-13T03:05:00Z" },
        }))
        .unwrap();
        match paid {
            GatewayStatus::Paid {
                paid_at,
                amount_idr,
                provider_payment_id,
                fee_idr,
            } => {
                assert_eq!(
                    amount_idr, 10_137,
                    "what landed is the unique amount, not what we asked for"
                );
                assert_eq!(provider_payment_id.as_deref(), Some("gw-7f2a"));
                assert_eq!(
                    fee_idr, None,
                    "the bridge reports no fee; a guess would corrupt the margin report"
                );
                assert_eq!(
                    paid_at,
                    DateTime::parse_from_rfc3339("2026-10-13T03:05:00Z")
                        .unwrap()
                        .with_timezone(&Utc)
                );
            }
            other => panic!("expected a paid status, got {other:?}"),
        }

        assert!(matches!(
            status_from_response(&json!({ "status": "settlement" })),
            Err(PaymentError::Invalid(_))
        ));
        assert!(
            matches!(
                status_from_response(&json!({ "status": "paid", "uniqueAmount": 10_137 })),
                Err(PaymentError::Invalid(_))
            ),
            "a paid answer with no settlement instant would decide A295 with our clock"
        );
    }

    /// The feed is a bare array and every entry is settled money, joined on the amount.
    #[test]
    fn the_transaction_feed_is_read_by_amount() {
        let raw = json!([
            {
                "id": "tx-1",
                "orderId": "gopay-99",
                "status": "settlement",
                "paymentType": "qris",
                "amount": 10_137,
                "realAmount": 10_037,
                "currency": "IDR",
                "time": "2026-10-13T03:05:00Z",
            },
            {
                "id": "tx-2",
                "orderId": "gopay-100",
                "status": "capture",
                "paymentType": "gopay",
                "amount": 25_004,
                "realAmount": 25_004,
                "currency": "IDR",
                "time": "2026-10-13T03:06:00Z",
            },
        ]);

        let list = transactions_from_response(&raw).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[0].amount_idr, 10_137,
            "the gross amount is what the buyer transferred, which is what we key on"
        );
        assert_eq!(list[0].transaction_id, "tx-1");
        assert_eq!(
            list[1].amount_idr, 25_004,
            "the fee is the provider's business; the join is on the gross"
        );

        assert!(
            matches!(
                transactions_from_response(&json!({ "transactions": [] })),
                Err(PaymentError::Invalid(_))
            ),
            "the internal contract's wrapper is not this gateway's shape"
        );
    }
}
