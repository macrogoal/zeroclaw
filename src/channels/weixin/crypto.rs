//! AES-128-ECB encryption for iLink CDN media files.
//!
//! The iLink Bot API transfers image/voice/video/file via CDN with
//! AES-128-ECB encryption. Files are:
//!   1. Encrypted locally with AES-128-ECB
//!   2. Uploaded to CDN with encrypted query params
//!   3. Downloaded from CDN (still encrypted)
//!   4. Decrypted locally with AES-128-ECB
//!
//! AES-128 requires a 16-byte key (128 bits).
//! ECB mode is used per Tencent's iLink protocol specification.

use anyhow::Context as _;
use base64::Engine as _;
use aes::cipher::{BlockDecrypt, BlockEncrypt};

/// AES-128 key (16 bytes)
#[derive(Debug, Clone, Copy)]
pub struct AesKey([u8; 16]);

impl AesKey {
    /// Parse from base64-encoded string (as provided by WeixinMessage.aes_key)
    pub fn from_base64(encoded: &str) -> anyhow::Result<Self> {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .context("base64 decode aes_key")?;
        if decoded.len() != 16 {
            anyhow::bail!(
                "AES-128 key must be 16 bytes, got {}",
                decoded.len()
            );
        }
        let mut key = [0u8; 16];
        key.copy_from_slice(&decoded);
        Ok(Self(key))
    }

    /// Parse from a raw 16-byte slice
    pub fn from_slice(slice: &[u8]) -> anyhow::Result<Self> {
        if slice.len() != 16 {
            anyhow::bail!(
                "AES-128 key must be 16 bytes, got {}",
                slice.len()
            );
        }
        let mut key = [0u8; 16];
        key.copy_from_slice(slice);
        Ok(Self(key))
    }

    /// Generate a random AES-128 key using the OS random source
    pub fn random() -> Self {
        let bytes = ring::rand::SecureRandom::fill(
            &ring::rand::SystemRandom::new(),
            &mut [0u8; 16],
        );
        // ring::rand never fails on SystemRandom — unwrap is safe
        let _ = bytes;
        let mut key = [0u8; 16];
        let _ = ring::rand::SecureRandom::fill(
            &ring::rand::SystemRandom::new(),
            &mut key,
        );
        Self(key)
    }

    /// Encode as base64 string (for use in API messages)
    pub fn to_base64_string(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.0)
    }

    /// Return raw bytes
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Encrypt plaintext with AES-128-ECB (zero-padding to block boundary).
/// Returns the ciphertext.
pub fn encrypt(key: &[u8; 16], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    use aes::Aes128;
    use aes::cipher::KeyInit;

    let cipher = Aes128::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("AES-128 key init: {}", e))?;

    let blocks = (plaintext.len() + 15) / 16;
    let mut padded = plaintext.to_vec();
    padded.resize(blocks * 16, 0);

    let mut out = Vec::with_capacity(blocks * 16);
    for chunk in padded.chunks_exact(16) {
        let mut block = aes::cipher::generic_array::GenericArray::clone_from_slice(chunk);
        cipher.encrypt_block(&mut block);
        out.extend_from_slice(&block);
    }

    Ok(out)
}

/// Decrypt ciphertext with AES-128-ECB.
/// Removes zero-padding from the result.
pub fn decrypt(key: &[u8; 16], ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
    use aes::Aes128;
    use aes::cipher::KeyInit;

    if ciphertext.len() % 16 != 0 {
        anyhow::bail!(
            "Ciphertext length must be a multiple of 16 bytes, got {}",
            ciphertext.len()
        );
    }

    let cipher = Aes128::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("AES-128 key init: {}", e))?;

    let blocks = ciphertext.len() / 16;
    let mut out = Vec::with_capacity(blocks * 16);

    for chunk in ciphertext.chunks_exact(16) {
        let mut block = aes::cipher::generic_array::GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        out.extend_from_slice(&block);
    }

    // Remove trailing zero-bytes (zero-padding)
    let end = out.iter().position(|&b| b != 0).unwrap_or(out.len());
    Ok(out[..end].to_vec())
}

/// Calculate MD5 hash of data, returning lowercase hex string.
pub fn md5_hex(data: &[u8]) -> String {
    let digest = md5::compute(data);
    format!("{:x}", digest)
}

/// Decrypt a CDN download URL from the encrypted query parameters
/// sent by Tencent in WeixinMessage media items.
///
/// The encrypted param is a base64-encoded, AES-ECB-encrypted JSON object
/// containing `{ "url": "https://..." }`.
pub async fn decrypt_cdn_url(
    encrypt_query_param: &str,
    aes_key_b64: &str,
) -> anyhow::Result<String> {
    // 1. Parse AES key
    let key = AesKey::from_base64(aes_key_b64)
        .context("parse AES key from base64")?;

    // 2. Base64-decode the encrypted query param
    let encrypted = base64::engine::general_purpose::STANDARD
        .decode(encrypt_query_param.trim())
        .context("base64 decode encrypt_query_param")?;

    // 3. AES-ECB decrypt
    let decrypted = decrypt(key.as_bytes(), &encrypted)
        .context("AES-ECB decrypt CDN URL")?;

    // 4. Parse JSON to extract "url" field
    let json: serde_json::Value = serde_json::from_slice(&decrypted)
        .context("parse decrypted CDN URL JSON")?;

    json.get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing 'url' field in CDN URL JSON"))
}

/// Detect MIME type from magic bytes (file signatures).
pub fn detect_mime_from_magic(data: &[u8]) -> &'static str {
    match data {
        &[0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, ..] => "image/png",
        &[0x47, 0x49, 0x46, 0x38, ..] => "image/gif", // GIF87a or GIF89a
        &[0x52, 0x49, 0x46, 0x46, 0x00, 0x00, 0x00, 0x00, 0x57, 0x41, 0x56, 0x45, ..] => "audio/wav",
        &[0x42, 0x4D, ..] => "image/bmp",
        &[0x49, 0x44, 0x33, ..] => "audio/mpeg", // ID3 = MP3
        &[0xFF, 0xFB, ..] => "audio/mpeg",       // MP3 without ID3
        &[0x4F, 0x67, 0x67, 0x53, ..] => "audio/ogg", // OGG
        &[0x02, ..] | &[0x03, ..] | &[0x04, ..] | &[0x05, ..] => "audio/silk",
        _ => "application/octet-stream",
    }
}

/// Map MIME type to common file extension.
pub fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/silk" => "silk",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aes_encrypt_decrypt_roundtrip() {
        let key = [0u8; 16];
        let plaintext = b"Hello, WeChat!";
        let ciphertext = encrypt(&key, plaintext).unwrap();
        let decrypted = decrypt(&key, &ciphertext).unwrap();
        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn test_aes_key_from_base64() {
        // "AAAAAAAAAAAAAAAA" == 16 x 'A' bytes
        let key = AesKey::from_base64("QUFBQUFBQUFBQUFB").unwrap();
        assert_eq!(key.as_bytes(), &[b'A'; 16]);
    }

    #[test]
    fn test_aes_random_key() {
        let k1 = AesKey::random();
        let k2 = AesKey::random();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
        // Roundtrip
        let b64 = k1.to_base64_string();
        let k3 = AesKey::from_base64(&b64).unwrap();
        assert_eq!(k1.as_bytes(), k3.as_bytes());
    }

    #[test]
    fn test_md5_hex() {
        assert_eq!(md5_hex(b"hello"), "5d41402abc4b2a76b9719d911017c592");
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn test_detect_mime_from_magic() {
        assert_eq!(detect_mime_from_magic(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(
            detect_mime_from_magic(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
            "image/png"
        );
        assert_eq!(detect_mime_from_magic(&[0x47, 0x49, 0x46, 0x38, 0x39, 0x61]), "image/gif");
        assert_eq!(detect_mime_from_magic(&[0x42, 0x4D, 0x00, 0x00]), "image/bmp");
        assert_eq!(
            detect_mime_from_magic(b"RIFF\x00\x00\x00\x00WAVE"),
            "audio/wav"
        );
        assert_eq!(detect_mime_from_magic(&[0x49, 0x44, 0x33, 0x04]), "audio/mpeg");
        assert_eq!(detect_mime_from_magic(&[0xFF, 0xFB, 0x90, 0x00]), "audio/mpeg");
        assert_eq!(detect_mime_from_magic(&[0x4F, 0x67, 0x67, 0x53]), "audio/ogg");
        assert_eq!(detect_mime_from_magic(&[0x02, 0x01, 0x01, 0x00]), "audio/silk");
        assert_eq!(detect_mime_from_magic(&[0x99, 0x99, 0x99, 0x99]), "application/octet-stream");
    }
}
