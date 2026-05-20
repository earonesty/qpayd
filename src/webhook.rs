use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn sign(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let payload = payload(timestamp, body);
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(&payload);
    format!(
        "t={timestamp},v1={}",
        hex::encode(mac.finalize().into_bytes())
    )
}

fn payload(timestamp: i64, body: &[u8]) -> Vec<u8> {
    let mut payload = timestamp.to_string().into_bytes();
    payload.push(b'.');
    payload.extend_from_slice(body);
    payload
}

pub async fn deliver(url: &str, secret: &str, body: &[u8]) -> anyhow::Result<()> {
    let timestamp = chrono::Utc::now().timestamp();
    let signature = sign(secret, timestamp, body);

    reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .header("qpayd-signature", signature)
        .body(body.to_vec())
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sign;

    #[test]
    fn signature_is_stable() {
        assert_eq!(
            sign("secret", 1700000000, br#"{"id":"evt_1"}"#),
            "t=1700000000,v1=af784f27423c462e20039559cd4264140f7b7ed4c9090e26fd663faa5eeb8dda"
        );
    }
}
