//! Check that a webhook really came from Unforged, then read it.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::error::{Error, Result};

/// How far a webhook's timestamp may be from now (5 minutes).
pub const DEFAULT_TOLERANCE: Duration = Duration::from_secs(300);

/// A webhook event.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookEvent {
    /// We may send an event more than once, so skip an id you've already handled.
    pub id: String,
    /// Such as `job.completed` or `loupe.check.completed`.
    pub event_type: String,
    pub created_at: String,
    pub data: Value,
    pub raw: Value,
}

fn bad(m: &str) -> Error {
    Error::WebhookSignature(m.into())
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Checks the `Unforged-Signature` header against the body exactly as it arrived.
pub fn verify_signature(
    payload: &[u8],
    header: Option<&str>,
    secret: &str,
    tolerance: Duration,
) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    verify_at(payload, header, secret, tolerance, now)
}

fn verify_at(
    payload: &[u8],
    header: Option<&str>,
    secret: &str,
    tolerance: Duration,
    now: u64,
) -> Result<()> {
    let header = header
        .filter(|h| !h.trim().is_empty())
        .ok_or_else(|| bad("the Unforged-Signature header is missing"))?;
    let mut t: Option<&str> = None;
    let mut sigs = Vec::new();
    for part in header.split(',') {
        if let Some((k, v)) = part.trim().split_once('=') {
            match k {
                "t" => t = Some(v),
                "v1" => sigs.push(v),
                _ => {}
            }
        }
    }
    let t = t
        .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| bad("the Unforged-Signature header has no timestamp"))?;
    if sigs.is_empty() {
        return Err(bad("the Unforged-Signature header has no signature"));
    }
    let decoded: Vec<Vec<u8>> = sigs
        .iter()
        .map(|s| hex_decode(s))
        .collect::<Option<_>>()
        .ok_or_else(|| bad("the Unforged-Signature header's signature isn't well formed"))?;
    let at: u64 = t
        .parse()
        .map_err(|_| bad("the Unforged-Signature header has no timestamp"))?;
    if now.abs_diff(at) > tolerance.as_secs() {
        return Err(bad("the webhook's timestamp is too far from now"));
    }
    let mac = || {
        let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .expect("HMAC takes a key of any length");
        m.update(t.as_bytes());
        m.update(b".");
        m.update(payload);
        m
    };
    if decoded.iter().any(|d| mac().verify_slice(d).is_ok()) {
        Ok(())
    } else {
        Err(bad("the webhook's signature doesn't match the secret"))
    }
}

/// Verifies the signature, then reads the event.
pub fn construct_event(payload: &[u8], header: Option<&str>, secret: &str) -> Result<WebhookEvent> {
    verify_signature(payload, header, secret, DEFAULT_TOLERANCE)?;
    parse(payload)
}

fn parse(payload: &[u8]) -> Result<WebhookEvent> {
    let v: Value = serde_json::from_slice(payload)
        .map_err(|e| Error::Decode(format!("the webhook's body isn't JSON: {e}")))?;
    Ok(WebhookEvent {
        id: crate::json::st(&v, "id"),
        event_type: crate::json::st(&v, "type"),
        created_at: crate::json::st(&v, "created_at"),
        data: crate::json::obj(&v, "data"),
        raw: v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &str, t: u64, body: &[u8]) -> String {
        let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        m.update(format!("{t}.").as_bytes());
        m.update(body);
        m.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    const BODY: &[u8] = br#"{"id":"evt_1","type":"job.completed","created_at":"x","data":{"job_id":"j"}}"#;

    #[test]
    fn a_good_signature_verifies() {
        let h = format!("t=1000,v1={}", sign("whsec", 1000, BODY));
        verify_at(BODY, Some(&h), "whsec", DEFAULT_TOLERANCE, 1100).unwrap();
        let e = parse(BODY).unwrap();
        assert_eq!(e.event_type, "job.completed");
        assert_eq!(e.data["job_id"], "j");
    }

    #[test]
    fn any_of_several_signatures_will_do() {
        let h = format!("t=1000,v1={},v1={}", "0".repeat(64), sign("whsec", 1000, BODY));
        verify_at(BODY, Some(&h), "whsec", DEFAULT_TOLERANCE, 1000).unwrap();
    }

    #[test]
    fn bad_signatures_are_refused() {
        let good = sign("whsec", 1000, BODY);
        let cases = [
            None,
            Some("".to_string()),
            Some(format!("v1={good}")),
            Some("t=1000".to_string()),
            Some("t=1000,v1=zz".to_string()),
            Some(format!("t=1000,v1={}", good.to_uppercase())),
            Some(format!("t=1000,v1={}", sign("other", 1000, BODY))),
            Some(format!("t=1000,v1={good}é")),
        ];
        for h in cases {
            let r = verify_at(BODY, h.as_deref(), "whsec", DEFAULT_TOLERANCE, 1000);
            assert!(matches!(r, Err(Error::WebhookSignature(_))), "{h:?}");
        }
        let h = format!("t=1000,v1={good}");
        assert!(verify_at(BODY, Some(&h), "whsec", DEFAULT_TOLERANCE, 1301).is_err());
        assert!(verify_at(b"{}", Some(&h), "whsec", DEFAULT_TOLERANCE, 1000).is_err());
    }
}
