
// ═════════════════════════════════════════════════════════════════════════════
// Phase 2 — Voice (stub implementations)
// ═════════════════════════════════════════════════════════════════════════════

impl WeXinChannel {
    /// Download voice (SILK → WAV) - stub
    async fn download_voice(&self, encrypt_param: &str, aes_key_b64: &str) -> anyhow::Result<(String, String)> {
        // TODO: Implement using decrypt_cdn_url + AES decrypt + ffmpeg SILK→WAV
        Err(anyhow::anyhow!("Voice download not implemented"))
    }
    
    /// Upload voice (WAV → SILK) - stub  
    async fn upload_voice(&self, to_user_id: &str, file_path: &std::path::Path) -> anyhow::Result<(String, String)> {
        // TODO: Implement using ffmpeg encode + AES encrypt + CDN upload
        Err(anyhow::anyhow!("Voice upload not implemented"))
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Phase 3 — File & Video (stub implementations)
// ═════════════════════════════════════════════════════════════════════════════

impl WeXinChannel {
    /// Download generic file - stub
    async fn download_generic_media(&self, encrypt_param: &str, aes_key_b64: &str, suggested_ext: &str) -> anyhow::Result<(String, Option<String>)> {
        // TODO: Implement using decrypt_cdn_url + AES decrypt + MIME detection
        Err(anyhow::anyhow!("File download not implemented"))
    }
    
    /// Download video with thumbnail - stub
    async fn download_video(&self, video_param: &str, video_key_b64: &str, thumb_param: Option<&str>, thumb_key_b64: Option<&str>) -> anyhow::Result<(String, Option<String>)> {
        // TODO: Implement
        Err(anyhow::anyhow!("Video download not implemented"))
    }
    
    /// Upload file - stub
    async fn upload_file(&self, to_user_id: &str, file_path: &std::path::Path) -> anyhow::Result<(String, String)> {
        // TODO: Implement using AES encrypt + CDN upload
        Err(anyhow::anyhow!("File upload not implemented"))
    }
    
    /// Upload video with thumbnail - stub
    async fn upload_video(&self, to_user_id: &str, file_path: &std::path::Path) -> anyhow::Result<(String, String, Option<(String, String)>)> {
        // TODO: Implement
        Err(anyhow::anyhow!("Video upload not implemented"))
    }
}