use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Generates an authentic Stripe webhook signature:
/// Format: `t=<timestamp>,v1=<hmac_sha256(secret, "$timestamp.$payload")>`
pub fn sign_stripe_webhook(payload: &[u8], secret: &str, timestamp: i64) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("Invalid HMAC secret: {}", e))?;

    let prefix = format!("{}.", timestamp);
    mac.update(prefix.as_bytes());
    mac.update(payload);

    let result = mac.finalize();
    let hex_sig = hex::encode(result.into_bytes());

    Ok(format!("t={},v1={}", timestamp, hex_sig))
}

/// Generates an authentic GitHub webhook signature:
/// Format: `sha256=<hmac_sha256(secret, payload)>`
pub fn sign_github_webhook(payload: &[u8], secret: &str) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("Invalid HMAC secret: {}", e))?;

    mac.update(payload);
    let result = mac.finalize();
    let hex_sig = hex::encode(result.into_bytes());

    Ok(format!("sha256={}", hex_sig))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stripe_signature_format() {
        let payload = br#"{"id":"evt_test_123"}"#;
        let secret = "whsec_test_secret";
        let timestamp = 1700000000;

        let sig = sign_stripe_webhook(payload, secret, timestamp).unwrap();
        assert!(sig.starts_with("t=1700000000,v1="));
        assert_eq!(sig.split(',').count(), 2);
    }

    #[test]
    fn test_github_signature_format() {
        let payload = br#"{"action":"opened"}"#;
        let secret = "secret_gh";

        let sig = sign_github_webhook(payload, secret).unwrap();
        assert!(sig.starts_with("sha256="));
    }
}
