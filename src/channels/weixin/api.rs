//! Tencent iLink Bot API client.
//!
//! Reference: https://github.com/Tencent/openclaw-weixin
//! Protocol: iLink Bot Protocol (official Tencent, non-reverse-engineered)
//!
//! This module provides a typed HTTP client for the iLink Bot REST API.
//! All communication is HTTPS; bearer-token auth; AES-128-ECB for media CDN.

pub use super::error::{ILinkErrorCode, ILinkErrorResponse};

use anyhow::Context as _;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const ILINK_API_BASE: &str = "https://ilinkapi.weixin.qq.com";

/// Generate a random uint32 as base64 for the X-WECHAT-UIN header.
/// This is a per-request nonce required by the iLink protocol.
fn random_uin_base64() -> String {
    let mut bytes = [0u8; 4];
    let _ = ring::rand::SecureRandom::fill(
        &ring::rand::SystemRandom::new(),
        &mut bytes,
    );
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ─────────────────────────────────────────────────────────────────────────────
// Request / Response types
// ─────────────────────────────────────────────────────────────────────────────

// getUpdates ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GetUpdatesReq<'a> {
    #[serde(rename = "get_updates_buf")]
    get_updates_buf: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct GetUpdatesResp {
    pub ret: i32,
    pub errcode: Option<i32>,
    pub errmsg: Option<String>,
    pub msgs: Vec<WeixinMessage>,
    #[serde(rename = "get_updates_buf")]
    pub get_updates_buf: Option<String>,
    #[serde(rename = "longpolling_timeout_ms")]
    pub longpolling_timeout_ms: Option<u64>,
}

// WeixinMessage ──────────────────────────────────────────────────────────────

/// An incoming message from a WeChat user via the iLink Bot API.
#[derive(Debug, Clone, Deserialize)]
pub struct WeixinMessage {
    /// Message sequence number
    pub seq: Option<u64>,
    /// Unique message ID
    #[serde(rename = "message_id")]
    pub message_id: Option<u64>,
    /// Sender's user ID (wxuin)
    #[serde(rename = "from_user_id")]
    pub from_user_id: Option<String>,
    /// Receiver's user ID (bot's wxuin)
    #[serde(rename = "to_user_id")]
    pub to_user_id: Option<String>,
    /// Creation timestamp in milliseconds
    #[serde(rename = "create_time_ms")]
    pub create_time_ms: Option<u64>,
    /// Session/conversation ID
    #[serde(rename = "session_id")]
    pub session_id: Option<String>,
    /// Message type: `1` = USER, `2` = BOT
    #[serde(rename = "message_type")]
    pub message_type: Option<u8>,
    /// Message state: `0` = NEW, `1` = GENERATING, `2` = FINISH
    #[serde(rename = "message_state")]
    pub message_state: Option<u8>,
    /// Content items (text, image, voice, file, video)
    #[serde(rename = "item_list")]
    pub item_list: Option<Vec<MessageItem>>,
    /// Context token — must be passed back in sendMessage for accurate threading
    #[serde(rename = "context_token")]
    pub context_token: Option<String>,
}

impl WeixinMessage {
    /// Returns true if this is a message FROM a user (not from the bot itself)
    pub fn is_from_user(&self) -> bool {
        self.message_type == Some(1)
    }

    /// Returns true if this is a NEW message (not a mid-generation state update)
    pub fn is_new(&self) -> bool {
        self.message_state == Some(0)
    }
}

// MessageItem ────────────────────────────────────────────────────────────────

/// A content item within a WeixinMessage.
///
/// The `type` field uses Tencent's integer codes:
///   1 = Text   2 = Image   3 = Voice   4 = File   5 = Video
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum MessageItem {
    #[serde(rename = "1")]
    Text {
        #[serde(rename = "text_item")]
        text_item: TextItem,
    },
    #[serde(rename = "2")]
    Image {
        #[serde(rename = "image_item")]
        image_item: ImageItem,
    },
    #[serde(rename = "3")]
    Voice {
        #[serde(rename = "voice_item")]
        voice_item: VoiceItem,
    },
    #[serde(rename = "4")]
    File {
        #[serde(rename = "file_item")]
        file_item: FileItem,
    },
    #[serde(rename = "5")]
    Video {
        #[serde(rename = "video_item")]
        video_item: VideoItem,
    },
}

impl MessageItem {
    /// Construct a text message item for outbound messages
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text {
            text_item: TextItem {
                text: content.into(),
            },
        }
    }

    /// Construct an image message item for outbound messages
    pub fn image(encrypt_query_param: impl Into<String>, aes_key: impl Into<String>) -> Self {
        Self::Image {
            image_item: ImageItem {
                encrypt_query_param: Some(encrypt_query_param.into()),
                aes_key: Some(aes_key.into()),
            },
        }
    }

    /// Construct a voice message item for outbound messages
    pub fn voice(encrypt_query_param: impl Into<String>, aes_key: impl Into<String>) -> Self {
        Self::Voice {
            voice_item: VoiceItem {
                encrypt_query_param: Some(encrypt_query_param.into()),
                aes_key: Some(aes_key.into()),
            },
        }
    }

    /// Returns true if this is a text item
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    /// Extract text content if this is a text item
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text_item } => Some(&text_item.text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TextItem {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageItem {
    #[serde(rename = "encrypt_query_param")]
    pub encrypt_query_param: Option<String>,
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceItem {
    #[serde(rename = "encrypt_query_param")]
    pub encrypt_query_param: Option<String>,
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileItem {
    #[serde(rename = "encrypt_query_param")]
    pub encrypt_query_param: Option<String>,
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VideoItem {
    #[serde(rename = "encrypt_query_param")]
    pub encrypt_query_param: Option<String>,
    pub aes_key: Option<String>,
    /// Video thumbnail
    pub thumb: Option<ImageItem>,
}

// sendMessage ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct SendMsgReq<'a> {
    msg: SendMsgPayload<'a>,
}

#[derive(Debug, Serialize)]
struct SendMsgPayload<'a> {
    #[serde(rename = "to_user_id")]
    pub to_user_id: &'a str,
    #[serde(rename = "context_token")]
    pub context_token: &'a str,
    #[serde(rename = "item_list")]
    pub item_list: &'a [MessageItem],
}

// getConfig ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GetConfigReq<'a> {
    #[serde(rename = "ilink_user_id")]
    pub ilink_user_id: &'a str,
    #[serde(rename = "context_token")]
    pub context_token: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct GetConfigResp {
    pub ret: i32,
    /// Base64-encoded typing ticket, used in sendTyping requests
    #[serde(rename = "typing_ticket")]
    pub typing_ticket: Option<String>,
}

// sendTyping ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct SendTypingReq<'a> {
    #[serde(rename = "ilink_user_id")]
    pub ilink_user_id: &'a str,
    #[serde(rename = "typing_ticket")]
    pub typing_ticket: &'a str,
    /// 1 = typing, 2 = cancel typing
    pub status: u8,
}

/// Typing indicator status values
#[derive(Debug, Clone, Copy)]
pub enum TypingStatus {
    Typing = 1,
    Cancel = 2,
}

// getUploadUrl ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct UploadParams {
    pub filekey: String,
    /// 1=IMAGE, 2=VIDEO, 3=FILE
    #[serde(rename = "media_type")]
    pub media_type: u8,
    #[serde(rename = "to_user_id")]
    pub to_user_id: String,
    /// Original file plaintext size in bytes
    pub rawsize: u64,
    /// MD5 of plaintext file
    #[serde(rename = "rawfilemd5")]
    pub rawfilemd5: String,
    /// Ciphertext size after AES-128-ECB encryption
    pub filesize: u64,
    /// Thumbnail original size (for images/videos)
    #[serde(rename = "thumb_rawsize")]
    pub thumb_rawsize: Option<u64>,
    /// Thumbnail MD5
    #[serde(rename = "thumb_rawfilemd5")]
    pub thumb_rawfilemd5: Option<String>,
    /// Thumbnail ciphertext size
    #[serde(rename = "thumb_filesize")]
    pub thumb_filesize: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct UploadUrlResp {
    /// Encrypted upload parameters (passed as query string when PUTing to CDN)
    #[serde(rename = "upload_param")]
    pub upload_param: String,
    /// Encrypted thumbnail upload parameters
    #[serde(rename = "thumb_upload_param")]
    pub thumb_upload_param: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP Client
// ─────────────────────────────────────────────────────────────────────────────

/// HTTP client for Tencent's iLink Bot API.
#[derive(Clone)]
pub struct WeXinApiClient {
    bot_token: String,
    client: reqwest::Client,
}

impl WeXinApiClient {
    /// Create a new iLink API client with the given bot token.
    /// The token is obtained via QR code OAuth (see OpenClaw's `@tencent-weixin/openclaw-weixin`).
    pub fn new(bot_token: String) -> Self {
        let client = crate::config::build_runtime_proxy_client("channel.weixin");
        Self { bot_token, client }
    }

    /// Expose the HTTP client for CDN downloads and other direct HTTP operations.
    pub fn http_client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Build the HTTP headers required by the iLink Bot API.
    fn make_headers(&self) -> reqwest::header::HeaderMap {
        use reqwest::header::*;
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, "application/json".parse().unwrap());
        // iLink requires this exact header value
        h.insert("AuthorizationType", "ilink_bot_token".parse().unwrap());
        h.insert(
            AUTHORIZATION,
            format!("Bearer {}", self.bot_token).parse().unwrap(),
        );
        // Per-request random nonce (uint32 base64-encoded)
        h.insert("X-WECHAT-UIN", random_uin_base64().parse().unwrap());
        h
    }

    // ── API methods ─────────────────────────────────────────────────────────

    /// Long-poll for new messages from WeChat users.
    ///
    /// On first call pass `cursor = None`. The response includes a new cursor
    /// (`get_updates_buf`) that must be passed to subsequent calls.
    ///
    /// The server holds the request open until:
    ///   - New messages arrive (returns immediately with messages)
    ///   - `timeout_ms` elapses (returns with empty `msgs`)
    ///   - An error occurs (errcode in response)
    ///
    /// # Errors
    ///
    /// Returns an error with semantic meaning:
    /// - `SessionTimeout (-14)`: Cursor expired, reset to empty
    /// - `InvalidToken (-20)`: Need to re-authenticate
    /// - `RateLimited (-100)`: Too many requests, wait before retry
    pub async fn get_updates(
        &self,
        cursor: Option<&str>,
        timeout_ms: u64,
    ) -> anyhow::Result<GetUpdatesResp> {
        let body = GetUpdatesReq {
            get_updates_buf: cursor.unwrap_or(""),
        };

        let resp = self
            .client
            .post(format!("{}/getupdates", ILINK_API_BASE))
            .headers(self.make_headers())
            .json(&body)
            .timeout(Duration::from_millis(timeout_ms + 5_000))
            .send()
            .await
            .map_err(|e| {
                // Classify network errors
                if e.is_timeout() {
                    anyhow::anyhow!(ILinkErrorCode::SessionTimeout.description())
                } else if e.is_connect() {
                    anyhow::anyhow!("iLink connection failed: {}", e)
                } else {
                    anyhow::anyhow!("iLink getUpdates HTTP request failed: {}", e)
                }
            })?;

        // Check HTTP status
        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "iLink getUpdates HTTP error {}: {}",
                status,
                err_body
            ));
        }

        let result: GetUpdatesResp = resp
            .json()
            .await
            .context("iLink getUpdates parse response failed")?;

        // Handle session timeout specially
        if result.errcode == Some(-14) {
            tracing::debug!("WeXin: session timeout (errcode -14), resetting cursor");
            return Ok(GetUpdatesResp {
                get_updates_buf: Some(String::new()),
                ..result
            });
        }

        // Handle invalid token
        if result.errcode == Some(-20) {
            tracing::warn!("WeXin: invalid token (errcode -20), need re-auth");
            return Err(anyhow::anyhow!(
                "{}. Please run: openclaw channels login --channel openclaw-weixin",
                ILinkErrorCode::InvalidToken.description()
            ));
        }

        // Check for other errors
        if result.ret != 0 {
            let code = ILinkErrorCode::from_code(result.errcode.unwrap_or(result.ret));
            return Err(anyhow::anyhow!(
                "iLink getUpdates error ({}): {}",
                result.errcode.unwrap_or(result.ret),
                code.description()
            ));
        }

        Ok(result)
    }

    /// Send a message (text, image, etc.) to a WeChat user.
    ///
    /// `context_token` should be the value from the incoming message being replied to.
    /// For proactive (bot-initiated) messages, pass an empty string.
    ///
    /// # Errors
    ///
    /// - `MessageTooLong (-301)`: Message exceeds limit
    /// - `ContentBlocked (-500)`: Content filtered by moderation
    /// - `InvalidToken (-20)`: Need to re-authenticate
    pub async fn send_message(
        &self,
        to_user_id: &str,
        context_token: &str,
        item_list: &[MessageItem],
    ) -> anyhow::Result<()> {
        let body = SendMsgReq {
            msg: SendMsgPayload {
                to_user_id,
                context_token,
                item_list,
            },
        };

        let resp = self
            .client
            .post(format!("{}/sendmessage", ILINK_API_BASE))
            .headers(self.make_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow::anyhow!("iLink sendMessage timeout")
                } else {
                    anyhow::anyhow!("iLink sendMessage HTTP error: {}", e)
                }
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            // Try to parse as JSON for structured error
            if let Ok(err_resp) = serde_json::from_str::<ILinkErrorResponse>(&err_body) {
                let code = err_resp.error_code();
                return Err(anyhow::anyhow!(
                    "iLink sendMessage error ({}): {} — {}",
                    err_resp.errcode.unwrap_or(status.as_u16() as i32),
                    code.description(),
                    err_resp.errmsg.unwrap_or_default()
                ));
            }
            return Err(anyhow::anyhow!(
                "iLink sendMessage HTTP error {}: {}",
                status,
                err_body
            ));
        }

        Ok(())
    }

    /// Get account configuration, including the `typing_ticket` needed for
    /// the sendTyping indicator.
    ///
    /// # Errors
    /// - `InvalidToken (-20)`: Need to re-authenticate
    /// - `UserNotFound (-200)`: User ID does not exist or is blocked
    pub async fn get_config(&self, user_id: &str) -> anyhow::Result<GetConfigResp> {
        let body = GetConfigReq {
            ilink_user_id: user_id,
            context_token: None,
        };

        let resp = self
            .client
            .post(format!("{}/getconfig", ILINK_API_BASE))
            .headers(self.make_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                anyhow::anyhow!("iLink getConfig HTTP error: {}", e)
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "iLink getConfig HTTP error {}: {}",
                status, err_body
            ));
        }

        let result: GetConfigResp = resp
            .json()
            .await
            .context("iLink getConfig parse response failed")?;

        if result.ret != 0 {
            let code = ILinkErrorCode::from_code(result.ret);
            return Err(anyhow::anyhow!(
                "iLink getConfig error (ret={}): {}",
                result.ret, code.description()
            ));
        }

        Ok(result)
    }

    /// Send a typing indicator (or cancel it) to a user.
    ///
    /// Requires a `typing_ticket` obtained from `get_config()`.
    /// Tencent's client shows "typing..." while status=1 is active.
    ///
    /// Failures are logged but not propagated — typing is non-critical.
    pub async fn send_typing(
        &self,
        user_id: &str,
        typing_ticket: &str,
        status: TypingStatus,
    ) -> anyhow::Result<()> {
        let body = SendTypingReq {
            ilink_user_id: user_id,
            typing_ticket: typing_ticket,
            status: status as u8,
        };

        let resp = self
            .client
            .post(format!("{}/sendtyping", ILINK_API_BASE))
            .headers(self.make_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!("WeXin: sendTyping HTTP error (non-critical): {}", e);
                anyhow::anyhow!("iLink sendTyping HTTP error: {}", e)
            })?;

        if !resp.status().is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            tracing::debug!(
                "WeXin: sendTyping failed (non-critical): {}",
                err_body
            );
            // Don't fail on typing errors — they're cosmetic
            return Ok(());
        }

        Ok(())
    }

    /// Get pre-signed CDN upload parameters for sending media files.
    ///
    /// Call this before uploading a file, then use the returned `upload_param`
    /// as the query string when PUTing the encrypted file to the CDN.
    ///
    /// # Errors
    /// - `FileTooLarge (-303)`: File exceeds size limit
    /// - `InvalidMediaType (-302)`: Unsupported media format
    /// - `UploadFailed (-400)`: CDN returned error
    pub async fn get_upload_url(&self, params: &UploadParams) -> anyhow::Result<UploadUrlResp> {
        let resp = self
            .client
            .post(format!("{}/getuploadurl", ILINK_API_BASE))
            .headers(self.make_headers())
            .json(params)
            .send()
            .await
            .map_err(|e| {
                anyhow::anyhow!("iLink getUploadUrl HTTP error: {}", e)
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let code = match status.as_u16() {
                413 => ILinkErrorCode::FileTooLarge,
                415 => ILinkErrorCode::InvalidMediaType,
                _ => ILinkErrorCode::UploadFailed,
            };
            return Err(anyhow::anyhow!(
                "iLink getUploadUrl error ({}): {}",
                status, code.description()
            ));
        }

        let result: UploadUrlResp = resp
            .json()
            .await
            .context("iLink getUploadUrl parse response failed")?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_item_text_factory() {
        let item = MessageItem::text("hello");
        assert!(item.is_text());
        assert_eq!(item.as_text(), Some("hello"));
    }

    #[test]
    fn test_message_item_image_factory() {
        let item = MessageItem::image("encrypted_param", "base64_aes_key");
        assert!(matches!(item, MessageItem::Image { .. }));
    }

    #[test]
    fn test_weixin_message_helpers() {
        let user_msg = WeixinMessage {
            seq: Some(1),
            message_id: Some(123),
            from_user_id: Some("user_abc".into()),
            to_user_id: Some("bot_xyz".into()),
            create_time_ms: Some(1_000_000_000_000),
            session_id: Some("sess_1".into()),
            message_type: Some(1),
            message_state: Some(0),
            item_list: None,
            context_token: Some("ctx_token".into()),
        };
        assert!(user_msg.is_from_user());
        assert!(user_msg.is_new());

        let bot_msg = WeixinMessage {
            message_type: Some(2),
            message_state: Some(2),
            ..Default::default()
        };
        assert!(!bot_msg.is_from_user());
    }

    impl Default for WeixinMessage {
        fn default() -> Self {
            Self {
                seq: None,
                message_id: None,
                from_user_id: None,
                to_user_id: None,
                create_time_ms: None,
                session_id: None,
                message_type: None,
                message_state: None,
                item_list: None,
                context_token: None,
            }
        }
    }
}
