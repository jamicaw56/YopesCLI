//! `/generate`: an image model call against an OpenAI-compatible
//! `/images/generations` endpoint, returning the PNG bytes.
//!
//! The reply is either base64 in the JSON (`b64_json`) or a URL to fetch; both
//! shapes are handled because providers differ on which one they send.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::Deserialize;

use crate::config::AuthKind;

/// The model `/generate` asks for when the user did not name one.
pub const DEFAULT_MODEL: &str = "gpt-image-1";

/// The image size asked for: square, and a size every image endpoint accepts.
const SIZE: &str = "1024x1024";

/// `<base>/images/generations`, whatever trailing slash the base came with.
pub fn url_for(base_url: &str) -> String {
    format!(
        "{}/images/generations",
        base_url.trim().trim_end_matches('/')
    )
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    data: Vec<Datum>,
}

#[derive(Deserialize)]
struct Datum {
    #[serde(default)]
    b64_json: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

/// The image bytes from a generation reply, or the reason there are none.
///
/// Synchronous so the shape of the two replies is testable without a network.
pub fn decode(body: &str) -> Result<Vec<u8>> {
    let reply: Reply = serde_json::from_str(body).context("reading the image reply")?;
    let Some(datum) = reply.data.into_iter().next() else {
        bail!("the image endpoint returned no image");
    };
    if let Some(encoded) = datum.b64_json.as_deref().filter(|text| !text.is_empty()) {
        return base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .context("decoding the image data");
    }
    match datum.url.as_deref().filter(|url| !url.is_empty()) {
        Some(url) => Ok(url.as_bytes().to_vec()),
        None => bail!("the image reply carried neither b64_json nor a url"),
    }
}

/// Ask the endpoint for one image and return its bytes.
///
/// A reply that only carries a URL is followed once: providers that hand out a
/// link do not inline the pixels.
pub async fn generate(
    base_url: &str,
    auth: AuthKind,
    key: Option<&str>,
    model: &str,
    prompt: &str,
) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        // Image models are slow: a minute is a normal wait, not a hang.
        .timeout(Duration::from_secs(180))
        .user_agent(concat!("yopes/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating the image client")?;
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
        "size": SIZE,
    });
    let mut request = client.post(url_for(base_url)).json(&body);
    if let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) {
        request = match auth {
            AuthKind::Bearer | AuthKind::Jwt => request.bearer_auth(key),
            AuthKind::XApiKey => request.header("x-api-key", key),
            AuthKind::None => request,
        };
    }
    let sent = request
        .send()
        .await
        .context("contacting the image endpoint")?
        .error_for_status()
        .context("generating the image")?;
    let bytes = sent.bytes().await.context("reading the image reply")?;
    let text = String::from_utf8_lossy(&bytes);
    // The image itself travels in the JSON body, never as the response body:
    // a PNG here means the endpoint answered something we do not understand.
    match decode(&text) {
        Ok(decoded) => {
            if decoded.starts_with(b"http") {
                let url = String::from_utf8_lossy(&decoded).to_string();
                let fetched = client
                    .get(&url)
                    .send()
                    .await
                    .context("fetching the generated image")?
                    .error_for_status()
                    .context("downloading the generated image")?;
                return Ok(fetched
                    .bytes()
                    .await
                    .context("reading the generated image")?
                    .to_vec());
            }
            Ok(decoded)
        }
        Err(error) => Err(error),
    }
}

/// The file name an image is saved under: `image-<stamp>.png`, with a counter
/// so a second run in the same second never overwrites the first.
pub fn file_name(dir: &std::path::Path, stamp: &str) -> String {
    let mut candidate = format!("image-{stamp}.png");
    let mut counter = 1;
    while dir.join(&candidate).exists() {
        candidate = format!("image-{stamp}-{counter}.png");
        counter += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endpoint_sits_below_the_base_url() {
        assert_eq!(
            url_for("https://api.example.com/v1/"),
            "https://api.example.com/v1/images/generations"
        );
        assert_eq!(
            url_for("https://api.example.com/v1"),
            "https://api.example.com/v1/images/generations"
        );
    }

    #[test]
    fn base64_and_url_replies_both_resolve() {
        // "hello" in base64.
        let bytes = decode(r#"{"data":[{"b64_json":"aGVsbG8="}]}"#).expect("base64 reply");
        assert_eq!(bytes, b"hello");

        let bytes =
            decode(r#"{"data":[{"url":"https://img.example.com/a.png"}]}"#).expect("url reply");
        assert_eq!(bytes, b"https://img.example.com/a.png");

        assert!(decode(r#"{"data":[]}"#).is_err());
        assert!(decode(r#"{"data":[{}]}"#).is_err());
    }

    #[test]
    fn a_saved_image_never_overwrites_another() {
        let dir = crate::temp::unique_dir("yopes-generate-name");
        assert_eq!(
            file_name(&dir, "20261125-120000"),
            "image-20261125-120000.png"
        );
        std::fs::write(dir.join("image-20261125-120000.png"), b"x").unwrap();
        assert_eq!(
            file_name(&dir, "20261125-120000"),
            "image-20261125-120000-1.png"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The whole call against a local stand-in for an image endpoint: the URL,
    /// the JSON body and the base64 reply are all exercised without a network.
    #[tokio::test]
    async fn a_generation_round_trip_returns_the_image_bytes() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = vec![0u8; 8192];
            let read = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..read]).to_string();
            let body = r#"{"data":[{"b64_json":"aGVsbG8="}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).expect("write reply");
            request
        });

        let bytes = generate(
            &format!("http://127.0.0.1:{port}/v1"),
            AuthKind::None,
            None,
            "some-image-model",
            "a red bicycle",
        )
        .await
        .expect("generated image");
        assert_eq!(bytes, b"hello");

        let request = server.join().expect("server thread");
        assert!(
            request.starts_with("POST /v1/images/generations "),
            "{request}"
        );
        assert!(
            request.contains("\"model\":\"some-image-model\""),
            "{request}"
        );
        assert!(
            request.contains("\"prompt\":\"a red bicycle\""),
            "{request}"
        );
    }
}
