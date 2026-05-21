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

pub async fn deliver(
    url: &str,
    secret: &str,
    event_id: &str,
    event_type: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let timestamp = chrono::Utc::now().timestamp();
    let signature = sign(secret, timestamp, body);

    reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .header("qpayd-event-id", event_id)
        .header("qpayd-event-type", event_type)
        .header("qpayd-signature", signature)
        .body(body.to_vec())
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::{deliver, sign};

    #[test]
    fn signature_is_stable() {
        assert_eq!(
            sign("secret", 1700000000, br#"{"id":"evt_1"}"#),
            "t=1700000000,v1=af784f27423c462e20039559cd4264140f7b7ed4c9090e26fd663faa5eeb8dda"
        );
    }

    #[tokio::test]
    async fn deliver_sends_event_headers_and_signature() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0; 4096];
            let bytes = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .await
                .unwrap();
            request
        });

        deliver(
            &url,
            "secret",
            "evt_invoice_1_settled",
            "invoice.settled",
            br#"{"id":"evt_invoice_1_settled"}"#,
        )
        .await
        .unwrap();

        let request = server.await.unwrap();
        assert!(request.contains("qpayd-event-id: evt_invoice_1_settled"));
        assert!(request.contains("qpayd-event-type: invoice.settled"));
        assert!(request.contains("qpayd-signature: t="));
    }
}
