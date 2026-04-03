//! iLink Bot API error handling.
//!
//! Provides structured error types for the iLink Bot Protocol,
//! mapping numeric error codes to semantic meanings.

use serde::{Deserialize, Serialize};
use std::fmt;

/// iLink Bot API error codes.
///
/// These error codes are returned by the iLink API server
/// in the `errcode` field of responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum ILinkErrorCode {
    /// Success (not an error)
    Success = 0,

    /// Generic error - check errmsg for details
    GenericError = -1,

    /// Session timeout - cursor has expired, reset and retry
    SessionTimeout = -14,

    /// Invalid token - need to re-authenticate
    InvalidToken = -20,

    /// Rate limited - too many requests
    RateLimited = -100,

    /// User not found / blocked
    UserNotFound = -200,

    /// Message too long
    MessageTooLong = -301,

    /// Invalid media type
    InvalidMediaType = -302,

    /// Media file too large
    FileTooLarge = -303,

    /// Upload failed
    UploadFailed = -400,

    /// Content blocked by Tencent moderation
    ContentBlocked = -500,

    /// Network error (client-side)
    NetworkError = -900,

    /// Unknown error code
    Unknown = i32::MIN,
}

impl ILinkErrorCode {
    /// Parse error code from i32.
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Success,
            -1 => Self::GenericError,
            -14 => Self::SessionTimeout,
            -20 => Self::InvalidToken,
            -100 => Self::RateLimited,
            -200 => Self::UserNotFound,
            -301 => Self::MessageTooLong,
            -302 => Self::InvalidMediaType,
            -303 => Self::FileTooLarge,
            -400 => Self::UploadFailed,
            -500 => Self::ContentBlocked,
            _ => Self::Unknown,
        }
    }

    /// Check if this error is retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::SessionTimeout
                | Self::RateLimited
                | Self::NetworkError
                | Self::UploadFailed
        )
    }

    /// Check if this error requires re-authentication.
    pub fn requires_reauth(&self) -> bool {
        matches!(self, Self::InvalidToken)
    }

    /// Get recommended retry delay in milliseconds.
    pub fn recommended_retry_delay_ms(&self) -> Option<u64> {
        match self {
            Self::SessionTimeout => Some(0), // Immediate retry with empty cursor
            Self::RateLimited => Some(60_000), // Wait 1 minute
            Self::NetworkError => Some(5_000), // Wait 5 seconds
            Self::UploadFailed => Some(10_000), // Wait 10 seconds
            _ => None,
        }
    }

    /// Get human-readable description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::GenericError => "Generic error",
            Self::SessionTimeout => "Session timeout - cursor expired, please reset",
            Self::InvalidToken => "Invalid or expired token - please re-authenticate",
            Self::RateLimited => "Rate limited - too many requests",
            Self::UserNotFound => "User not found or blocked",
            Self::MessageTooLong => "Message exceeds maximum length",
            Self::InvalidMediaType => "Invalid or unsupported media type",
            Self::FileTooLarge => "File exceeds maximum size limit",
            Self::UploadFailed => "File upload failed",
            Self::ContentBlocked => "Content blocked by moderation",
            Self::NetworkError => "Network error",
            Self::Unknown => "Unknown error",
        }
    }

    /// Get suggested action for this error.
    pub fn suggested_action(&self) -> &'static str {
        match self {
            Self::Success => "No action needed",
            Self::GenericError => "Check error message for details",
            Self::SessionTimeout => "Reset cursor to empty and retry",
            Self::InvalidToken => "Run 'openclaw channels login --channel openclaw-weixin'",
            Self::RateLimited => "Wait before retrying, implement backoff",
            Self::UserNotFound => "Verify user ID is correct",
            Self::MessageTooLong => "Split message into smaller parts",
            Self::InvalidMediaType => "Use supported format: jpg, png, mp4, etc.",
            Self::FileTooLarge => "Compress file or use smaller resolution",
            Self::UploadFailed => "Retry upload with exponential backoff",
            Self::ContentBlocked => "Modify content to comply with ToS",
            Self::NetworkError => "Check network connectivity",
            Self::Unknown => "Check Tencent documentation",
        }
    }
}

impl fmt::Display for ILinkErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

impl From<i32> for ILinkErrorCode {
    fn from(code: i32) -> Self {
        Self::from_code(code)
    }
}

/// iLink Bot API error response.
#[derive(Debug, Clone, Deserialize)]
pub struct ILinkErrorResponse {
    /// Top-level return code (0 = success)
    pub ret: i32,
    /// Specific error code
    pub errcode: Option<i32>,
    /// Human-readable error message
    pub errmsg: Option<String>,
}

impl ILinkErrorResponse {
    /// Check if response indicates success.
    pub fn is_success(&self) -> bool {
        self.ret == 0 && self.errcode.unwrap_or(0) == 0
    }

    /// Get the parsed error code.
    pub fn error_code(&self) -> ILinkErrorCode {
        ILinkErrorCode::from_code(self.errcode.unwrap_or(self.ret))
    }

    /// Convert to an anyhow error.
    pub fn to_error(&self) -> anyhow::Error {
        let code = self.error_code();
        let msg = self.errmsg.as_deref().unwrap_or("Unknown error");

        if self.errcode.is_some() {
            anyhow::anyhow!(
                "iLink error (ret={}, errcode={}): {} — {}",
                self.ret,
                self.errcode.unwrap(),
                code.description(),
                msg
            )
        } else {
            anyhow::anyhow!(
                "iLink error (ret={}): {}",
                self.ret,
                msg
            )
        }
    }
}

/// Error category for classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Transient error - retry may succeed
    Transient,
    /// Permanent error - do not retry
    Permanent,
    /// Auth error - need to re-authenticate
    Auth,
    /// Rate limit - need to wait
    RateLimit,
    /// Content error - blocked by moderation
    Content,
}

impl From<&ILinkErrorCode> for ErrorCategory {
    fn from(code: &ILinkErrorCode) -> Self {
        match code {
            ILinkErrorCode::Success => Self::Permanent, // Not used
            ILinkErrorCode::GenericError => Self::Transient,
            ILinkErrorCode::SessionTimeout => Self::Transient,
            ILinkErrorCode::InvalidToken => Self::Auth,
            ILinkErrorCode::RateLimited => Self::RateLimit,
            ILinkErrorCode::UserNotFound => Self::Permanent,
            ILinkErrorCode::MessageTooLong => Self::Permanent,
            ILinkErrorCode::InvalidMediaType => Self::Permanent,
            ILinkErrorCode::FileTooLarge => Self::Permanent,
            ILinkErrorCode::UploadFailed => Self::Transient,
            ILinkErrorCode::ContentBlocked => Self::Content,
            ILinkErrorCode::NetworkError => Self::Transient,
            ILinkErrorCode::Unknown => Self::Permanent,
        }
    }
}

/// Retry policy based on error code.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial delay in milliseconds
    pub initial_delay_ms: u64,
    /// Maximum delay in milliseconds
    pub max_delay_ms: u64,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 5,
            initial_delay_ms: 1_000,
            max_delay_ms: 60_000,
            backoff_multiplier: 2.0,
        }
    }
}

impl RetryPolicy {
    /// Calculate delay for a given attempt number.
    pub fn delay_for_attempt(&self, attempt: u32) -> u64 {
        let delay = self.initial_delay_ms as f64
            * self.backoff_multiplier.powi(attempt as i32);
        delay.min(self.max_delay_ms as f64) as u64
    }

    /// Check if more retries are allowed.
    pub fn should_retry(&self, attempt: u32, error: &ILinkErrorCode) -> bool {
        attempt < self.max_retries && error.is_retryable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_parsing() {
        assert_eq!(ILinkErrorCode::from_code(0), ILinkErrorCode::Success);
        assert_eq!(ILinkErrorCode::from_code(-14), ILinkErrorCode::SessionTimeout);
        assert_eq!(ILinkErrorCode::from_code(-20), ILinkErrorCode::InvalidToken);
        assert_eq!(ILinkErrorCode::from_code(-999), ILinkErrorCode::Unknown);
    }

    #[test]
    fn test_is_retryable() {
        assert!(ILinkErrorCode::SessionTimeout.is_retryable());
        assert!(ILinkErrorCode::RateLimited.is_retryable());
        assert!(!ILinkErrorCode::InvalidToken.is_retryable());
        assert!(!ILinkErrorCode::MessageTooLong.is_retryable());
    }

    #[test]
    fn test_requires_reauth() {
        assert!(ILinkErrorCode::InvalidToken.requires_reauth());
        assert!(!ILinkErrorCode::SessionTimeout.requires_reauth());
    }

    #[test]
    fn test_retry_policy() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_for_attempt(0), 1_000);
        assert_eq!(policy.delay_for_attempt(1), 2_000);
        assert_eq!(policy.delay_for_attempt(2), 4_000);
        assert_eq!(policy.delay_for_attempt(10), 60_000); // capped
    }

    #[test]
    fn test_error_category() {
        let cat: ErrorCategory = (&ILinkErrorCode::SessionTimeout).into();
        assert_eq!(cat, ErrorCategory::Transient);

        let cat: ErrorCategory = (&ILinkErrorCode::InvalidToken).into();
        assert_eq!(cat, ErrorCategory::Auth);
    }
}
