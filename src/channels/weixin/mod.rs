//! Personal WeChat channel via Tencent iLink Bot Protocol.
//!
//! Implements the [`Channel`] trait for ZeroClaw's multi-channel system.
//! Supports: text, image, voice, file, video messages.
//!
//! **Protocol:** iLink Bot API (Tencent official)
//! **Endpoint:** `https://ilinkapi.weixin.qq.com`
//! **Scope:** Private chat only (no groups)
//! **Auth:** Bearer token from QR code OAuth
//!
//! ## Architecture
//!
//! ```text
//! WeChat App (Phone)
//!   ↓ (iLink Bot Protocol, HTTPS)
//! Tencent iLink Backend
//!   ↓ (HTTP JSON + Bearer Token)
//! WeXinChannel
//!   ├─ listen()          → long-poll loop (getUpdates)
//!   ├─ send()             → send text/media (sendMessage)
//!   ├─ start_typing()    → typing indicator (sendTyping)
//!   └─ process_message()  → convert to ChannelMessage
//!   ↓
//! ZeroClaw Agent Loop
//! ```
//!
//! ## Limitations
//!
//! - **Private chat only** — iLink Bot API does not support group chats
//! - **Single bot per account** — one WeChat account = one bot
//! - **No native OAuth** — token must be obtained via OpenClaw's QR login:
//!   `openclaw channels login --channel openclaw-weixin`

mod api;
pub mod crypto;

use super::media_pipeline::MediaAttachment;
use super::traits::{Channel, ChannelMessage, SendMessage};
use self::api::{MessageItem, WeixinMessage};
use self::crypto::AesKey;
use anyhow::Context;
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{sleep, Duration};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Channel struct
// ─────────────────────────────────────────────────────────────────────────────

/// WeChat iLink Bot channel.
///
/// Manages a long-lived session with the Tencent iLink Bot API,
/// polling for incoming messages and sending outbound responses.
pub struct WeXinChannel {
    /// iLink API HTTP client
    api: self::api::WeXinApiClient,
    /// Allowlist of user IDs (wxuin). Empty = deny all, "*" = allow all
    allowed_users: Vec<String>,
    /// Server-suggested long-poll timeout in milliseconds (default 35000)
    timeout_ms: u64,
    /// Sync cursor — must be passed to every getUpdates call
    cursor: RwLock<Option<String>>,
    /// Cached typing tickets: user_id → ticket (from getConfig)
    typing_tickets: RwLock<HashMap<String, String>>,
    /// Context tokens per user: user_id → context_token (from incoming messages)
    context_tokens: RwLock<HashMap<String, String>>,
    /// Exponential backoff delay in ms (starts at 1000, caps at 60000)
    reconnect_delay_ms: RwLock<u64>,
    /// Runtime state machine
    state: RwLock<WeXinState>,
    /// Max image download size in bytes (default 20 MB)
    max_image_size_bytes: u64,
}

/// Channel runtime state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeXinState {
    /// Disconnected — initial state
    Disconnected,
    /// Connecting / polling
    Running,
    /// Shutdown requested
    ShuttingDown,
}

impl WeXinChannel {
    /// Create a new WeXin channel.
    ///
    /// `bot_token` is obtained via OpenClaw's QR login:
    /// ```bash
    /// openclaw channels login --channel openclaw-weixin
    /// ```
    pub fn new(
        bot_token: String,
        allowed_users: Vec<String>,
        long_poll_timeout_ms: u64,
    ) -> Self {
        Self {
            api: self::api::WeXinApiClient::new(bot_token),
            allowed_users,
            timeout_ms: long_poll_timeout_ms,
            cursor: RwLock::new(None),
            typing_tickets: RwLock::new(HashMap::new()),
            context_tokens: RwLock::new(HashMap::new()),
            reconnect_delay_ms: RwLock::new(1_000),
            state: RwLock::new(WeXinState::Disconnected),
            max_image_size_bytes: 20 * 1024 * 1024,
        }
    }

    /// Check if a user ID is allowed to send messages.
    fn is_user_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.is_empty()
            || self.allowed_users.iter().any(|u| u == "*" || u == user_id)
    }

    /// Reset exponential backoff to initial value
    async fn reset_backoff(&self) {
        let mut d = self.reconnect_delay_ms.write().await;
        *d = 1_000;
    }

    /// Double reconnect delay with 60-second cap
    async fn backoff(&self) -> u64 {
        let mut d = self.reconnect_delay_ms.write().await;
        let new_delay = (*d * 2).min(60_000);
        *d = new_delay;
        new_delay
    }

    // ── Attachment marker parser ───────────────────────────────────────────

    /// Parse `[IMAGE:path]` markers from message content and upload images.
    /// Returns a list of MessageItems with text + uploaded images.
    async fn parse_attachment_markers(
        &self,
        content: &str,
        recipient: &str,
    ) -> anyhow::Result<Vec<MessageItem>> {
        use self::api::MessageItem;

        let mut items = Vec::new();
        let mut cleaned_parts = Vec::new();
        let mut chars = content.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '[' {
                let mut found_end = false;
                let mut inner = String::new();
                while let Some(&next) = chars.peek() {
                    if next == ']' {
                        found_end = true;
                        let _ = chars.next(); // consume ]
                        break;
                    }
                    inner.push(chars.next().unwrap());
                }

                if found_end {
                    if let Some(path) = inner.strip_prefix("IMAGE:") {
                        match self.upload_image(recipient, std::path::Path::new(path)).await {
                            Ok((param, key)) => {
                                items.push(MessageItem::image(param, key));
                            }
                            Err(e) => {
                                tracing::warn!("WeXin: image upload failed: {}", e);
                                cleaned_parts.push(format!("[IMAGE:{}]", path));
                            }
                        }
                    } else if inner.starts_with("VOICE:") {
                        // TODO(Phase 2): upload voice
                        cleaned_parts.push(format!("[{}]", inner));
                    } else if inner.starts_with("FILE:") {
                        // TODO(Phase 3): upload file
                        cleaned_parts.push(format!("[{}]", inner));
                    } else if inner.starts_with("VIDEO:") {
                        // TODO(Phase 3): upload video
                        cleaned_parts.push(format!("[{}]", inner));
                    } else {
                        cleaned_parts.push(format!("[{}]", inner));
                    }
                } else {
                    // No closing ], keep as-is
                    cleaned_parts.push(String::from(ch));
                    cleaned_parts.push(inner);
                }
            } else {
                cleaned_parts.push(String::from(ch));
            }
        }

        let text = cleaned_parts.join("");
        if !text.trim().is_empty() {
            items.insert(0, MessageItem::text(text));
        }

        Ok(items)
    }

    // ── Image upload ─────────────────────────────────────────────────────

    /// Upload a local image file to Tencent CDN and return encrypted params.
    ///
    /// Flow:
    /// 1. Read file from disk
    /// 2. Generate random AES-128 key
    /// 3. Encrypt file with AES-128-ECB
    /// 4. Calculate MD5 of plaintext
    /// 5. Call getUploadUrl → get encrypted upload params + CDN URL
    /// 6. PUT encrypted file to CDN
    /// 7. Return (encrypt_query_param, aes_key) for sendMessage
    pub async fn upload_image(
        &self,
        to_user_id: &str,
        file_path: &std::path::Path,
    ) -> anyhow::Result<(String, String)> {
        use self::api::UploadParams;

        // 1. Read file
        let data = tokio::fs::read(file_path).await
            .with_context(|| format!("read image file {:?}", file_path))?;
        let raw_size = data.len() as u64;
        let raw_md5 = self::crypto::md5_hex(&data);

        // 2. Generate random AES-128 key
        let key = self::crypto::AesKey::random();
        let aes_key_b64 = key.to_base64_string();

        // 3. Encrypt
        let encrypted = self::crypto::encrypt(key.as_bytes(), &data)
            .context("AES-128-ECB encrypt image")?;
        let encrypted_size = encrypted.len() as u64;

        // 4. Generate thumbnail (if image crate is available)
        let thumb_result: Option<(Vec<u8>, String, u64, String)> =
            match self.generate_thumbnail(&data).await {
                Ok((thumb_data, thumb_md5)) => {
                    let thumb_enc = self::crypto::encrypt(key.as_bytes(), &thumb_data)?;
                    let thumb_size = thumb_enc.len() as u64;
                    Some((thumb_enc, thumb_md5.clone(), thumb_size, thumb_md5))
                }
                Err(e) => {
                    tracing::warn!("WeXin: thumbnail generation failed: {}", e);
                    None
                }
            };

        // 5. Call getUploadUrl
        let params = UploadParams {
            filekey: Uuid::new_v4().to_string(),
            media_type: 1, // IMAGE
            to_user_id: to_user_id.to_string(),
            rawsize: raw_size,
            rawfilemd5: raw_md5.clone(),
            filesize: encrypted_size,
            thumb_rawsize: thumb_result.as_ref().map(|p| p.2),
            thumb_rawfilemd5: thumb_result.as_ref().map(|p| p.3.clone()),
            thumb_filesize: thumb_result.as_ref().map(|p| p.0.len() as u64),
        };

        let upload_resp = self
            .api
            .get_upload_url(&params)
            .await
            .context("iLink getUploadUrl failed")?;

        // 6. PUT encrypted file to CDN
        let cdn_url = upload_resp.upload_param.clone();
        let put_resp = self
            .api
            .http_client()
            .put(&cdn_url)
            .header("Content-Type", "application/octet-stream")
            .body(encrypted)
            .send()
            .await
            .context("CDN PUT failed for image")?;

        if !put_resp.status().is_success() {
            let err = put_resp.text().await.unwrap_or_default();
            anyhow::bail!("CDN image upload failed: {}", err);
        }

        tracing::debug!(
            "WeXin: uploaded image {} ({} bytes)",
            file_path.display(),
            data.len()
        );

        // 7. Return encrypt_query_param + aes_key
        Ok((upload_resp.upload_param, aes_key_b64))
    }

    /// Generate a JPEG thumbnail for an image. Returns (bytes, md5_hex).
    async fn generate_thumbnail(
        &self,
        image_data: &[u8],
    ) -> anyhow::Result<(Vec<u8>, String)> {
        let img = image::load_from_memory(image_data)
            .with_context(|| "decode image for thumbnail")?;
        let thumb = img.thumbnail(200, 200);

        let mut buf = Vec::new();
        thumb
            .write_to(
                &mut std::io::Cursor::new(&mut buf),
                image::ImageFormat::Jpeg,
            )
            .with_context(|| "encode thumbnail as JPEG")?;

        let md5 = self::crypto::md5_hex(&buf);
        Ok((buf, md5))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Channel trait implementation
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl Channel for WeXinChannel {
    fn name(&self) -> &'static str {
        "weixin"
    }

    /// Send a text message (and optionally images) to a WeChat user.
    ///
    /// Supports `[IMAGE:path]` markers in the content for sending images.
    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let context_token = self
            .context_tokens
            .read()
            .await
            .get(&message.recipient)
            .cloned()
            .unwrap_or_default();

        let items = self
            .parse_attachment_markers(&message.content, &message.recipient)
            .await?;

        self.api
            .send_message(&message.recipient, &context_token, &items)
            .await
            .with_context(|| format!("WeXin: failed to send to {}", message.recipient))?;

        tracing::debug!(
            "WeXin: sent {} item(s) to {}",
            items.len(),
            message.recipient
        );

        Ok(())
    }

    /// Start the long-poll listener loop.
    ///
    /// Runs until the channel is shut down. Handles:
    /// 1. Long-polling getUpdates
    /// 2. Converting WeixinMessage → ChannelMessage
    /// 3. Exponential backoff on errors
    async fn listen(&self, tx: mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        {
            let mut s = self.state.write().await;
            *s = WeXinState::Running;
        }

        tracing::info!("WeXin: starting iLink listener (timeout={}ms)", self.timeout_ms);

        loop {
            if *self.state.read().await == WeXinState::ShuttingDown {
                tracing::info!("WeXin: shutdown requested");
                break;
            }

            let cursor = self.cursor.read().await.clone();
            let timeout = self.timeout_ms;

            match self.api.get_updates(cursor.as_deref(), timeout).await {
                Ok(resp) => {
                    self.reset_backoff().await;
                    {
                        let mut c = self.cursor.write().await;
                        *c = resp.get_updates_buf;
                    }
                    for wx_msg in resp.msgs {
                        if let Err(e) = self.process_message(tx.clone(), &wx_msg).await {
                            tracing::warn!("WeXin: process_message error: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("WeXin: getUpdates error: {}", e);
                    let delay = self.backoff().await;
                    tracing::info!("WeXin: backing off {}ms", delay);
                    sleep(Duration::from_millis(delay)).await;
                }
            }
        }

        Ok(())
    }

    /// Health check — verify the iLink API is reachable.
    async fn health_check(&self) -> bool {
        self.api.get_updates(None, 5_000).await.is_ok()
    }

    /// Start typing indicator for the given recipient.
    async fn start_typing(&self, recipient: &str) -> anyhow::Result<()> {
        self.typing_indicator(recipient, self::api::TypingStatus::Typing)
            .await
    }

    /// Stop typing indicator for the given recipient.
    async fn stop_typing(&self, recipient: &str) -> anyhow::Result<()> {
        self.typing_indicator(recipient, self::api::TypingStatus::Cancel)
            .await
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Message processing
// ─────────────────────────────────────────────────────────────────────────────

impl WeXinChannel {
    /// Process a single incoming WeixinMessage.
    async fn process_message(
        &self,
        tx: mpsc::Sender<ChannelMessage>,
        wx_msg: &WeixinMessage,
    ) -> anyhow::Result<()> {
        // Ignore bot messages
        if !wx_msg.is_from_user() {
            return Ok(());
        }

        let sender = wx_msg
            .from_user_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WeXin: missing from_user_id"))?;

        if !self.is_user_allowed(sender) {
            tracing::debug!("WeXin: blocked user {}", sender);
            return Ok(());
        }

        // Ignore mid-generation state updates
        if !wx_msg.is_new() {
            return Ok(());
        }

        let (content, attachments) = self.extract_content(wx_msg).await?;

        if content.trim().is_empty() && attachments.is_empty() {
            return Ok(());
        }

        // Store context_token for reply threading
        if let Some(ref ctx) = wx_msg.context_token {
            let mut tokens = self.context_tokens.write().await;
            tokens.insert(sender.clone(), ctx.clone());
        }

        let msg = ChannelMessage {
            id: wx_msg
                .message_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            sender: sender.clone(),
            reply_target: sender.clone(),
            content,
            channel: "weixin".to_string(),
            timestamp: wx_msg.create_time_ms.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
            }),
            thread_ts: None,
            interruption_scope_id: None,
            attachments,
        };

        let msg_id = msg.id.clone();
        let item_count = wx_msg.item_list.as_ref().map_or(0, |v| v.len());

        tx.send(msg)
            .await
            .map_err(|e| anyhow::anyhow!("WeXin: mpsc send error: {}", e))?;

        tracing::debug!(
            "WeXin: processed msg id={} from={} items={}",
            msg_id,
            sender,
            item_count
        );

        Ok(())
    }

    /// Extract text and media attachments from a WeixinMessage.
    async fn extract_content(
        &self,
        wx_msg: &WeixinMessage,
    ) -> anyhow::Result<(String, Vec<MediaAttachment>)> {
        use self::api::MessageItem;

        let items = wx_msg
            .item_list
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WeXin: missing item_list"))?;

        let mut text_parts = Vec::new();
        let mut attachments = Vec::new();

        for item in items {
            match item {
                MessageItem::Text { text_item } => {
                    text_parts.push(text_item.text.clone());
                }
                MessageItem::Image { image_item } => {
                    match (
                        image_item.encrypt_query_param.as_deref(),
                        image_item.aes_key.as_deref(),
                    ) {
                        (Some(param), Some(key)) => {
                            match self.download_image(param, key).await {
                                Ok((path, mime, data)) => {
                                    text_parts.push(format!("[IMAGE:{}]", path));
                                    attachments.push(MediaAttachment {
                                        file_name: format!(
                                            "weixin_image.{}",
                                            self::crypto::mime_to_ext(&mime)
                                        ),
                                        data,
                                        mime_type: Some(mime),
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!("WeXin: image download failed: {}", e);
                                    text_parts.push("[Image]".to_string());
                                }
                            }
                        }
                        _ => {
                            text_parts.push("[Image]".to_string());
                        }
                    }
                }
                MessageItem::Voice { .. } => {
                    // TODO(Phase 2): SILK decode + transcription
                    text_parts.push("[Voice]".to_string());
                }
                MessageItem::File { .. } => {
                    // TODO(Phase 3): download generic file
                    text_parts.push("[File]".to_string());
                }
                MessageItem::Video { .. } => {
                    // TODO(Phase 3): download video + thumbnail
                    text_parts.push("[Video]".to_string());
                }
            }
        }

        Ok((text_parts.join("\n"), attachments))
    }

    /// Download and decrypt an image from the Tencent CDN.
    async fn download_image(
        &self,
        encrypt_param: &str,
        aes_key_b64: &str,
    ) -> anyhow::Result<(String, String, Vec<u8>)> {
        // Step 1: Decrypt the CDN URL
        let url = self::crypto::decrypt_cdn_url(encrypt_param, aes_key_b64)
            .await
            .context("decrypt CDN URL")?;

        // Step 2: Download encrypted blob
        let encrypted = self
            .api
            .http_client()
            .get(&url)
            .send()
            .await
            .with_context(|| format!("download CDN image from {}", url))?
            .bytes()
            .await
            .with_context(|| "read CDN image bytes")?
            .to_vec();

        // Step 3: Size check
        if encrypted.len() as u64 > self.max_image_size_bytes {
            anyhow::bail!(
                "WeXin: image too large ({} bytes, max {} bytes)",
                encrypted.len(),
                self.max_image_size_bytes
            );
        }

        // Step 4: AES-128-ECB decrypt
        let key = AesKey::from_base64(aes_key_b64).context("parse AES key")?;
        let decrypted =
            self::crypto::decrypt(key.as_bytes(), &encrypted).context("AES decrypt")?;

        // Step 5: Detect MIME from magic bytes
        let mime = self::crypto::detect_mime_from_magic(&decrypted);
        let ext = self::crypto::mime_to_ext(mime);

        // Step 6: Save to temp file
        let temp_dir = std::env::temp_dir();
        let filename = format!("weixin_img_{}.{}", Uuid::new_v4(), ext);
        let path = temp_dir.join(&filename);
        tokio::fs::write(&path, &decrypted)
            .await
            .with_context(|| format!("write temp image to {}", path.display()))?;

        tracing::debug!(
            "WeXin: downloaded image {} ({} bytes, {})",
            path.display(),
            decrypted.len(),
            mime
        );

        Ok((path.to_string_lossy().to_string(), mime.to_string(), decrypted))
    }

    /// Send a typing indicator to a user.
    async fn typing_indicator(
        &self,
        user_id: &str,
        status: self::api::TypingStatus,
    ) -> anyhow::Result<()> {
        let ticket = {
            if let Some(t) = self.typing_tickets.read().await.get(user_id).cloned() {
                t
            } else {
                let resp = self
                    .api
                    .get_config(user_id)
                    .await
                    .with_context(|| "WeXin: getConfig for typing_ticket")?;
                let t = resp.typing_ticket
                    .ok_or_else(|| anyhow::anyhow!("WeXin: no typing_ticket"))?;
                let mut m = self.typing_tickets.write().await;
                m.insert(user_id.to_string(), t.clone());
                t
            }
        };

        self.api
            .send_typing(user_id, &ticket, status)
            .await
            .with_context(|| format!("WeXin: sendTyping to {}", user_id))?;

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_allowlist_wildcard() {
        let ch = WeXinChannel::new("token".into(), vec!["*".into()], 35_000);
        assert!(ch.is_user_allowed("any_user"));
        assert!(ch.is_user_allowed("wxuin_123"));
    }

    #[test]
    fn test_user_allowlist_specific() {
        let ch = WeXinChannel::new(
            "token".into(),
            vec!["alice".into(), "bob".into()],
            35_000,
        );
        assert!(ch.is_user_allowed("alice"));
        assert!(ch.is_user_allowed("bob"));
        assert!(!ch.is_user_allowed("charlie"));
    }

    #[test]
    fn test_user_allowlist_empty_denies_all() {
        let ch = WeXinChannel::new("token".into(), vec![], 35_000);
        assert!(!ch.is_user_allowed("anyone"));
    }

    #[test]
    fn test_state_initial() {
        let ch = WeXinChannel::new("token".into(), vec![], 35_000);
        assert_eq!(*ch.state.try_read().unwrap(), WeXinState::Disconnected);
    }
}
