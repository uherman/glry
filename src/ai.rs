//! AI "describe this image" feature.
//!
//! Posts the current fullscreen image as a base64 data-URL to an
//! OpenAI-compatible chat-completions endpoint (default:
//! <https://api.swiftrouter.com/v1>) and returns the assistant's reply.
//!
//! Disabled unless `ai_enabled = true` is set in the user config *and* an
//! API token is available via the `SWIFTROUTER_API_KEY` environment
//! variable. Requests run on the rayon pool and post results back on an
//! mpsc channel the main event loop drains — the same shape
//! [`crate::thumbnail::ThumbWorker`] uses.

use anyhow::{Context, Result};
use base64::Engine;
use image::DynamicImage;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// Environment variable that holds the gateway bearer token.
pub const TOKEN_ENV: &str = "SWIFTROUTER_API_KEY";

/// Timeout for the gateway round-trip. Vision calls on a small model
/// typically settle in 2-5 s; 30 s leaves comfortable headroom without
/// hanging the spinner indefinitely if the provider stalls.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on the completion length. The overlay wraps text so a
/// chatty response is fine, but there's no point paying for a book.
const MAX_TOKENS: u32 = 400;

/// Settings controlling the AI describe feature. Parsed from the user
/// config file; defaults disable the feature.
#[derive(Debug, Clone)]
pub struct AiConfig {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://api.swiftrouter.com/v1".to_string(),
            model: "gpt-5.4-mini".to_string(),
        }
    }
}

/// System prompt used for every describe call. Deliberately terse — the
/// overlay is a single on-screen paragraph and users aren't reading an
/// essay.
const SYSTEM_PROMPT: &str = "You are describing images for a terminal \
image-viewer user. Write 2-4 short plain-prose sentences covering what's \
in the image, any notable text, and mood or style. Be concrete. No \
markdown, no bullet lists, no preamble.";

/// Completed describe job handed back to the main thread.
pub struct DescribeDone {
    pub path: PathBuf,
    pub result: Result<String>,
}

/// Background AI worker. Wraps an mpsc so describe results flow back on
/// the same drain-on-tick schedule as thumbnail loads.
pub struct AiWorker {
    tx: Sender<DescribeDone>,
    rx: Receiver<DescribeDone>,
}

impl AiWorker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }

    /// Dispatch a describe request. `image` should already be the decoded,
    /// possibly-downscaled full-resolution image so we aren't re-reading
    /// the file on the rayon thread.
    pub fn dispatch(&self, path: PathBuf, image: DynamicImage, cfg: AiConfig, token: String) {
        let tx = self.tx.clone();
        rayon::spawn(move || {
            let result = describe(&image, &cfg, &token);
            let _ = tx.send(DescribeDone { path, result });
        });
    }

    pub fn drain(&self) -> Vec<DescribeDone> {
        self.rx.try_iter().collect()
    }
}

impl Default for AiWorker {
    fn default() -> Self {
        Self::new()
    }
}

/// Downscale the supplied image (if needed) and POST it to the chat
/// endpoint, returning the trimmed assistant reply.
pub(crate) fn describe(image: &DynamicImage, cfg: &AiConfig, token: &str) -> Result<String> {
    let encoded = encode_png_for_upload(image).context("encoding image for AI")?;
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&encoded)
    );

    let body = serde_json::json!({
        "model": cfg.model,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": [
                { "type": "text", "text": "Describe this image." },
                { "type": "image_url", "image_url": { "url": data_url } },
            ]},
        ],
        "max_tokens": MAX_TOKENS,
    });

    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let resp = agent
        .post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_json(&body)
        .map_err(|e| match e {
            ureq::Error::Status(code, r) => {
                let body = r.into_string().unwrap_or_default();
                anyhow::anyhow!("AI gateway returned {code}: {}", body.trim())
            }
            ureq::Error::Transport(t) => anyhow::anyhow!("AI transport error: {t}"),
        })?;

    let json: serde_json::Value = resp.into_json().context("parsing AI response JSON")?;
    let content = json
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .context("AI response had no choices[0].message.content")?;
    Ok(content.trim().to_string())
}

/// Vision endpoints accept PNG data URLs; bigger images cost more tokens
/// without improving the description noticeably. Downscale to at most
/// 1024 px on the longest side before encoding.
fn encode_png_for_upload(image: &DynamicImage) -> Result<Vec<u8>> {
    const MAX_UPLOAD_DIM: u32 = 1024;
    let scaled = if image.width() > MAX_UPLOAD_DIM || image.height() > MAX_UPLOAD_DIM {
        image.thumbnail(MAX_UPLOAD_DIM, MAX_UPLOAD_DIM)
    } else {
        image.clone()
    };
    let mut buf = Vec::new();
    scaled
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .context("PNG encode")?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hits the real gateway; ignored by default so CI doesn't burn credits
    /// or require a token. Run with:
    ///
    ///   SWIFTROUTER_API_KEY=... cargo test --bin glry -- \
    ///     --ignored --nocapture describe_real_image
    #[test]
    #[ignore]
    fn describe_real_image() {
        let token = std::env::var("SWIFTROUTER_API_KEY").expect("SWIFTROUTER_API_KEY not set");
        let path = std::env::var("GLRY_TEST_IMAGE")
            .unwrap_or_else(|_| "/tmp/glry-test/test.png".to_string());
        let img = image::open(&path).expect("test image missing");
        let cfg = AiConfig::default();
        let out = describe(&img, &cfg, &token).expect("describe failed");
        println!("--- AI describe ---\n{out}\n-------------------");
        assert!(!out.is_empty());
    }

    #[test]
    fn encode_downscales_big_images() {
        let big = DynamicImage::new_rgb8(4000, 3000);
        let bytes = encode_png_for_upload(&big).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(decoded.width() <= 1024 && decoded.height() <= 1024);
    }

    #[test]
    fn encode_preserves_small_images() {
        let small = DynamicImage::new_rgb8(200, 150);
        let bytes = encode_png_for_upload(&small).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (200, 150));
    }
}
