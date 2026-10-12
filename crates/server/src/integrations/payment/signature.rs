//! The three callback signature schemes (A292), and nothing else.
//!
//! Pure: the caller hands over the headers, the **raw body** and the shared secret,
//! and gets `Ok(())` or [`PaymentError::Signature`]. It is separate from the client
//! so the route can verify before it parses — a body that fails the signature is
//! never deserialised, which is what keeps a forged payload out of the error path.
//!
//! Every comparison is constant time, through `hmac`'s `verify_slice` (which is
//! `subtle` underneath). `==` on a tag leaks the length of the matching prefix, which
//! is enough to recover a signature byte by byte given a retry loop. `shared_token`
//! is the scheme that makes this easy to get wrong, so it folds the presented value
//! into the same MAC comparison rather than writing a byte loop of its own.

use axum::http::HeaderMap;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;

use stvdy_core::traits::payment::PaymentError;

use crate::config::SignatureScheme;

type HmacSha256 = Hmac<Sha256>;

/// The MAC key used to fold a `shared_token` comparison into a constant-time one.
/// Domain-separated so a value that verified as a body tag can never verify as a
/// token.
const SHARED_TOKEN_CONTEXT: &[u8] = b"stvdy.qris.shared-token.v1";

/// Verifies a callback's signature.
///
/// `header_name` is the gateway's header, from `QRIS_SIGNATURE_HEADER`: the header
/// name is as much a gateway fact as the scheme is, so it is config rather than a
/// constant here.
///
/// Every failure is the same [`PaymentError::Signature`], with no detail: telling a
/// caller *which* check failed is telling an attacker what to fix. The one exception
/// is [`PaymentError::NotConfigured`] for an empty secret, which is our own
/// misconfiguration and must never be reported as a failed verification.
pub fn verify(
    scheme: SignatureScheme,
    headers: &HeaderMap,
    raw_body: &[u8],
    secret: &SecretString,
    header_name: &str,
) -> Result<(), PaymentError> {
    let secret = secret.expose_secret();
    if secret.is_empty() {
        return Err(PaymentError::NotConfigured);
    }

    let presented = headers
        .get(header_name)
        .and_then(|value| value.to_str().ok());

    match scheme {
        SignatureScheme::HmacSha256Hex => {
            let presented = presented
                .and_then(|value| hex::decode(value.trim()).ok())
                .ok_or(PaymentError::Signature)?;
            verify_tag(secret, raw_body, &presented)
        }
        SignatureScheme::HmacSha256Base64 => {
            use base64::Engine;
            let presented = presented
                .and_then(|value| {
                    base64::engine::general_purpose::STANDARD
                        .decode(value.trim())
                        .ok()
                })
                .ok_or(PaymentError::Signature)?;
            verify_tag(secret, raw_body, &presented)
        }
        // The gateway signs nothing: the header is the shared token itself. The
        // presented value is folded into a MAC keyed by the secret and compared to the
        // MAC of the secret, so the comparison stays inside `verify_slice` and the
        // token's length is the only thing the wire format already reveals.
        SignatureScheme::SharedToken => {
            let presented = presented.ok_or(PaymentError::Signature)?;
            let expected = tag(secret, SHARED_TOKEN_CONTEXT, secret.as_bytes())?;
            let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes())
                .map_err(|_| PaymentError::NotConfigured)?;
            mac.update(SHARED_TOKEN_CONTEXT);
            mac.update(presented.as_bytes());
            mac.verify_slice(&expected)
                .map_err(|_| PaymentError::Signature)
        }
    }
}

/// `HMAC-SHA256(secret, raw_body)` against the presented tag, constant time.
fn verify_tag(secret: &str, raw_body: &[u8], presented: &[u8]) -> Result<(), PaymentError> {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes())
        .map_err(|_| PaymentError::NotConfigured)?;
    mac.update(raw_body);
    mac.verify_slice(presented)
        .map_err(|_| PaymentError::Signature)
}

/// A MAC over `context || message`, for the one comparison that has no wire tag.
fn tag(secret: &str, context: &[u8], message: &[u8]) -> Result<Vec<u8>, PaymentError> {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes())
        .map_err(|_| PaymentError::NotConfigured)?;
    mac.update(context);
    mac.update(message);
    Ok(mac.finalize().into_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(name: &str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
            axum::http::HeaderValue::from_str(value).expect("a header value"),
        );
        headers
    }

    fn secret(raw: &str) -> SecretString {
        SecretString::from(raw.to_string())
    }

    fn hex_tag(secret: &str, body: &[u8]) -> String {
        let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes()).expect("a key");
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn base64_tag(secret: &str, body: &[u8]) -> String {
        use base64::Engine;
        let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes()).expect("a key");
        mac.update(body);
        base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
    }

    #[test]
    fn every_scheme_accepts_its_own_shape_and_rejects_the_others() {
        let body = br#"{"event_id":"evt_1"}"#;
        let name = "X-Callback-Signature";
        let token = secret("shared-token");

        let hex_value = hex_tag("shared-token", body);
        let base64_value = base64_tag("shared-token", body);

        assert!(
            verify(
                SignatureScheme::HmacSha256Hex,
                &headers(name, &hex_value),
                body,
                &token,
                name
            )
            .is_ok()
        );
        assert!(
            verify(
                SignatureScheme::HmacSha256Base64,
                &headers(name, &base64_value),
                body,
                &token,
                name
            )
            .is_ok()
        );
        assert!(
            verify(
                SignatureScheme::SharedToken,
                &headers(name, "shared-token"),
                body,
                &token,
                name
            )
            .is_ok()
        );

        // Each scheme against the *other two* shapes, which is what stops a
        // copy-paste making two arms the same code.
        assert!(matches!(
            verify(
                SignatureScheme::HmacSha256Hex,
                &headers(name, &base64_value),
                body,
                &token,
                name
            ),
            Err(PaymentError::Signature)
        ));
        assert!(matches!(
            verify(
                SignatureScheme::HmacSha256Hex,
                &headers(name, "shared-token"),
                body,
                &token,
                name
            ),
            Err(PaymentError::Signature)
        ));
        assert!(matches!(
            verify(
                SignatureScheme::HmacSha256Base64,
                &headers(name, &hex_value),
                body,
                &token,
                name
            ),
            Err(PaymentError::Signature)
        ));
        assert!(matches!(
            verify(
                SignatureScheme::SharedToken,
                &headers(name, &hex_value),
                body,
                &token,
                name
            ),
            Err(PaymentError::Signature)
        ));

        // A body swapped under a valid signature: the tag was over the *other* bytes.
        assert!(matches!(
            verify(
                SignatureScheme::HmacSha256Hex,
                &headers(name, &hex_value),
                br#"{"event_id":"evt_2"}"#,
                &token,
                name
            ),
            Err(PaymentError::Signature)
        ));

        // A missing header is a failure, never an empty-string match.
        assert!(matches!(
            verify(
                SignatureScheme::SharedToken,
                &HeaderMap::new(),
                body,
                &token,
                name
            ),
            Err(PaymentError::Signature)
        ));

        // A signature from another gateway's secret does not verify.
        assert!(matches!(
            verify(
                SignatureScheme::HmacSha256Hex,
                &headers(name, &hex_tag("another-secret", body)),
                body,
                &token,
                name
            ),
            Err(PaymentError::Signature)
        ));

        // An empty secret is our misconfiguration, and must not read as a bad signature.
        assert!(matches!(
            verify(
                SignatureScheme::HmacSha256Hex,
                &headers(name, &hex_value),
                body,
                &secret(""),
                name
            ),
            Err(PaymentError::NotConfigured)
        ));
    }

    #[test]
    fn the_configured_header_name_is_the_one_read() {
        let body = b"{}";
        let tag_value = hex_tag("k", body);

        assert!(
            verify(
                SignatureScheme::HmacSha256Hex,
                &headers("X-Other-Signature", &tag_value),
                body,
                &secret("k"),
                "X-Other-Signature"
            )
            .is_ok()
        );
        // The default header carrying the configured one's value must not verify: the
        // header name is part of the contract, not decoration.
        assert!(matches!(
            verify(
                SignatureScheme::HmacSha256Hex,
                &headers("X-Callback-Signature", &tag_value),
                body,
                &secret("k"),
                "X-Other-Signature"
            ),
            Err(PaymentError::Signature)
        ));
    }

    #[test]
    fn a_scheme_outside_the_closed_set_does_not_parse() {
        for scheme in SignatureScheme::ALL {
            assert_eq!(SignatureScheme::parse(scheme.as_str()), Some(scheme));
        }
        assert_eq!(SignatureScheme::parse("md5"), None);
        assert_eq!(SignatureScheme::parse(""), None);
        // Shorthands are not spellings: an operator's `hex` must be a boot failure
        // rather than a silent fallback to a scheme they did not choose.
        assert_eq!(SignatureScheme::parse("hex"), None);
        assert_eq!(SignatureScheme::parse("base64"), None);
    }
}
