//! WeChat iLink Bot authentication (Phase 4).
//!
//! Provides QR code login flow for personal WeChat via iLink Bot Protocol.
//!
//! # Flow
//! 1. User runs: `zeroclaw auth login weixin`
//! 2. ZeroClaw displays QR code in terminal (or opens browser)
//! 3. User scans with WeChat phone app
//! 4. ZeroClaw receives Bearer token
//! 5. Token saved to ~/.zeroclaw/auth.json

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// iLink Bot login endpoints.
/// Note: These are theoretical endpoints - actual implementation may differ.
/// The official OpenClaw plugin uses internal Tencent OAuth.
const ILINK_LOGIN_BASE: &str = "https://ilinkapi.weixin.qq.com";
const ILINK_QR_URL: &str = "https://open.weixin.qq.com/connect/qrconnect";

/// Login session state for QR code flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeixinLoginSession {
    /// Session ID for polling.
    pub session_id: String,
    /// QR code URL (embedded in base64 or external).
    pub qr_url: String,
    /// Timestamp when session was created.
    pub created_at: u64,
    /// Timestamp when session expires.
    pub expires_at: u64,
}

/// Result of successful QR login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeixinAuthCredentials {
    /// iLink Bot bearer token.
    pub bot_token: String,
    /// WeChat user ID (wxuin) of the bot.
    pub bot_wxuin: String,
    /// Token expiration timestamp (0 = no expiry).
    pub expires_at: u64,
}

/// WeChat login via QR code.
///
/// # Implementation Note
/// This is a simplified implementation. The actual iLink Bot authentication
/// flow uses Tencent's internal OAuth system which is not publicly documented.
///
/// For production use, users should:
/// 1. Run `openclaw channels login --channel openclaw-weixin`
/// 2. Copy the token from `~/.openclaw/credentials`
/// 3. Configure it in ZeroClaw via `~/.zeroclaw/auth.json`
pub struct WeixinAuth;

impl WeixinAuth {
    /// Initiate QR code login flow.
    ///
    /// Returns a login session with QR code URL to display to user.
    pub async fn start_login() -> Result<WeixinLoginSession> {
        // Note: The actual iLink QR login API is not publicly documented.
        // This implementation shows the structure; actual endpoint needs
        // reverse engineering or official documentation.
        //
        // For now, return a message directing users to OpenClaw.
        
        let session = WeixinLoginSession {
            session_id: Self::generate_session_id(),
            qr_url: format!(
                "{}?appid=wx_ilink_bot&redirect_uri={}&response_type=code&scope=snsapi_login",
                ILINK_QR_URL,
                urlencoding::encode("https://ilinkapi.weixin.qq.com/callback")
            ),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            expires_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() + 300, // 5 minute expiry
        };

        Ok(session)
    }

    /// Display QR code instructions to user.
    ///
    /// For terminals with QR support, displays the QR directly.
    /// Otherwise, prints the URL for manual opening.
    pub fn display_qr_instructions(session: &WeixinLoginSession) -> Result<()> {
        println!("\n╔══════════════════════════════════════════════════════════╗");
        println!("║          WeChat iLink Bot — QR Code Login               ║");
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║                                                          ║");
        println!("║  1. Open this URL in your browser:                       ║");
        println!("║                                                          ║");
        println!("║  {}  ║", session.qr_url);
        println!("║                                                          ║");
        println!("║  2. Scan the QR code with WeChat on your phone           ║");
        println!("║                                                          ║");
        println!("║  3. Confirm authorization in WeChat                     ║");
        println!("║                                                          ║");
        println!("║  Session ID: {}                          ║", &session.session_id[..16]);
        println!("║  Expires in: 5 minutes                                   ║");
        println!("║                                                          ║");
        println!("╚══════════════════════════════════════════════════════════╝\n");

        // Try to open browser automatically
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", &session.qr_url])
                .spawn();
            println!("Opening browser automatically...\n");
        }

        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg(&session.qr_url)
                .spawn();
            println!("Opening browser automatically...\n");
        }

        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(&session.qr_url)
                .spawn();
            println!("Opening browser automatically...\n");
        }

        Ok(())
    }

    /// Poll for login completion.
    ///
    /// Returns credentials when user has scanned and authorized.
    /// Returns None if still waiting.
    /// Returns error if session expired or was cancelled.
    pub async fn poll_login(session: &WeixinLoginSession) -> Result<Option<WeixinAuthCredentials>> {
        // Check if session has expired
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        if now >= session.expires_at {
            anyhow::bail!("Login session expired. Please run `zeroclaw auth login weixin` again.");
        }

        // Note: Actual polling implementation would call the iLink API
        // to check if the user has scanned. This is a placeholder.
        //
        // The endpoint would be something like:
        // POST https://ilinkapi.weixin.qq.com/auth/check
        // { "session_id": "..." }
        //
        // Response would contain the token when ready.

        // For now, return None to indicate polling is needed
        // In production, this would make an HTTP request
        Ok(None)
    }

    /// Alternative: Import token from OpenClaw credentials.
    ///
    /// Reads `~/.openclaw/credentials` and extracts the weixin token.
    pub fn import_from_openclaw() -> Result<Option<WeixinAuthCredentials>> {
        let openclaw_creds_path = {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .context("Could not find home directory")?;
            PathBuf::from(home).join(".openclaw").join("credentials")
        };

        if !openclaw_creds_path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&openclaw_creds_path)
            .context("Failed to read OpenClaw credentials")?;

        // Parse JSON and look for weixin/wechat entry
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            // OpenClaw credentials format:
            // { "weixin": { "bot_token": "...", "bot_wxuin": "..." } }
            if let Some(weixin) = json.get("weixin").or(json.get("openclaw-weixin")) {
                let bot_token = weixin
                    .get("bot_token")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_default();

                let bot_wxuin = weixin
                    .get("bot_wxuin")
                    .or(weixin.get("wxuin"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_default();

                if !bot_token.is_empty() {
                    return Ok(Some(WeixinAuthCredentials {
                        bot_token,
                        bot_wxuin,
                        expires_at: 0, // No expiry info available
                    }));
                }
            }
        }

        Ok(None)
    }

    /// Save credentials to ZeroClaw auth store.
    pub fn save_credentials(creds: &WeixinAuthCredentials) -> Result<()> {
        let auth_path = Self::auth_path()?;
        
        // Create parent directory if needed
        if let Some(parent) = auth_path.parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create auth directory")?;
        }

        // Read existing auth or create new
        let mut auth_data: serde_json::Value = if auth_path.exists() {
            let content = std::fs::read_to_string(&auth_path)
                .context("Failed to read auth file")?;
            serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        // Update weixin entry
        auth_data["weixin"] = serde_json::json!({
            "bot_token": creds.bot_token,
            "bot_wxuin": creds.bot_wxuin,
            "expires_at": creds.expires_at,
        });

        // Write back
        let content = serde_json::to_string_pretty(&auth_data)
            .context("Failed to serialize auth data")?;
        
        std::fs::write(&auth_path, content)
            .context("Failed to write auth file")?;

        println!("✓ WeChat iLink credentials saved to {}", auth_path.display());

        Ok(())
    }

    /// Load credentials from ZeroClaw auth store.
    pub fn load_credentials() -> Result<Option<WeixinAuthCredentials>> {
        let auth_path = Self::auth_path()?;
        
        if !auth_path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&auth_path)
            .context("Failed to read auth file")?;

        let json: serde_json::Value = serde_json::from_str(&content)
            .context("Failed to parse auth file")?;

        if let Some(weixin) = json.get("weixin") {
            let bot_token = weixin
                .get("bot_token")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();

            let bot_wxuin = weixin
                .get("bot_wxuin")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();

            let expires_at = weixin
                .get("expires_at")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            if !bot_token.is_empty() {
                return Ok(Some(WeixinAuthCredentials {
                    bot_token,
                    bot_wxuin,
                    expires_at,
                }));
            }
        }

        Ok(None)
    }

    /// Interactive login flow.
    ///
    /// Tries to import from OpenClaw first, then falls back to QR flow.
    pub async fn interactive_login() -> Result<WeixinAuthCredentials> {
        // Try importing from OpenClaw first
        if let Some(creds) = Self::import_from_openclaw()? {
            println!("\n✓ Found WeChat credentials in OpenClaw!");
            println!("  Token: {}...", &creds.bot_token[..20]);
            println!("\n  Importing to ZeroClaw...\n");
            
            Self::save_credentials(&creds)?;
            return Ok(creds);
        }

        // Start QR login flow
        println!("\nWeChat iLink Bot Login");
        println!("=======================");
        println!("\nNo existing credentials found.");
        println!("Starting QR code login flow...\n");

        let session = Self::start_login().await?;
        Self::display_qr_instructions(&session)?;

        println!("Waiting for authorization...\n");
        println!("Note: The actual iLink Bot QR login requires official");
        println!("      documentation from Tencent. For now, please use:");
        println!("\n      openclaw channels login --channel openclaw-weixin\n");

        // Placeholder: Would poll for completion
        // In production, this would loop checking the API
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            
            if let Some(creds) = Self::poll_login(&session).await? {
                Self::save_credentials(&creds)?;
                return Ok(creds);
            }

            // Check expiry
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            if now >= session.expires_at {
                anyhow::bail!("Login timed out. Please try again.");
            }
        }
    }

    // Helper functions

    fn generate_session_id() -> String {
        // Use ring for cryptographic random generation
        let mut bytes = [0u8; 16];
        let _ = ring::rand::SecureRandom::fill(
            &ring::rand::SystemRandom::new(),
            &mut bytes,
        );
        hex::encode(bytes)
    }

    fn auth_path() -> Result<PathBuf> {
        // Use the same pattern as ZeroClaw config module
        let config_dir = if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() {
                PathBuf::from(home).join(".zeroclaw")
            } else {
                Self::default_config_dir()?
            }
        } else {
            Self::default_config_dir()?
        };
        Ok(config_dir.join("auth.json"))
    }

    fn default_config_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .context("Could not find home directory")?;
        Ok(PathBuf::from(home).join(".zeroclaw"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_session_id() {
        let id = WeixinAuth::generate_session_id();
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_url_encoding() {
        // Simple URL encoding test
        let encoded = "hello world".replace(' ', "%20");
        assert_eq!(encoded, "hello%20world");
    }
}
