// ═════════════════════════════════════════════════════════════════════════════
// Phase 2 — Voice Messages (SILK codec)
// ═════════════════════════════════════════════════════════════════════════════
//
// WeChat voice messages use the SILK codec (same as Skype). WeChat sends the
// audio as encrypted SILK data via the CDN. To use the audio:
//
//   Receive path:
//     SILK blob (CDN) → AES decrypt → .silk file → ffmpeg → WAV/MP3 → transcription
//
//   Send path:
//     WAV/MP3 file → ffmpeg → SILK encode → AES encrypt → CDN upload → sendMessage
//
// Dependencies:
//   - ffmpeg must be installed and in PATH (discovered via `which` crate)
//   - `which::which("ffmpeg")` → PathBuf
//
// TODO(Phase 2): Pure Rust SILK decoder (no ffmpeg dependency)

impl WeXinChannel {
    /// Download a voice message from CDN and convert to WAV.
    ///
    /// Flow:
    /// 1. Decrypt CDN URL
    /// 2. Download encrypted SILK blob
    /// 3. AES-128-ECB decrypt
    /// 4. Save as .silk file
    /// 5. ffmpeg convert SILK → WAV (16kHz mono PCM)
    ///
    /// Returns `(wav_path, silk_path)` or just `(silk_path, silk_path)` if ffmpeg unavailable.
    async fn download_voice(
        &self,
        encrypt_param: &str,
        aes_key_b64: &str,
    ) -> anyhow::Result<(String, String)> {
        // Step 1: Decrypt CDN URL
        let url = self::crypto::decrypt_cdn_url(encrypt_param, aes_key_b64)
            .await
            .context("decrypt CDN URL for voice")?;

        // Step 2: Download encrypted blob
        let encrypted = self
            .api
            .http_client()
            .get(&url)
            .send()
            .await
            .with_context(|| "download voice from CDN")?
            .bytes()
            .await
            .with_context(|| "read voice bytes")?
            .to_vec();

        // Step 3: AES-128-ECB decrypt
        let key = AesKey::from_base64(aes_key_b64)
            .context("parse voice AES key")?;
        let decrypted = self::crypto::decrypt(key.as_bytes(), &encrypted)
            .context("AES decrypt voice")?;

        // Step 4: Save as .silk file
        let temp_dir = std::env::temp_dir();
        let silk_filename = format!("weixin_voice_{}.silk", Uuid::new_v4());
        let silk_path = temp_dir.join(&silk_filename);
        tokio::fs::write(&silk_path, &decrypted)
            .await
            .with_context(|| "write SILK file")?;

        tracing::debug!(
            "WeXin: downloaded voice {} bytes as {}",
            decrypted.len(),
            silk_path.display()
        );

        // Step 5: Convert SILK → WAV with ffmpeg
        match self.convert_silk_to_wav(&silk_path).await {
            Ok(wav_path) => Ok((wav_path, silk_path.to_string_lossy().to_string())),
            Err(e) => {
                tracing::warn!(
                    "WeXin: ffmpeg SILK→WAV failed (ffmpeg may not be installed): {}",
                    e
                );
                // Return the raw SILK path if conversion fails
                Ok((silk_path.to_string_lossy().to_string(), silk_path.to_string_lossy().to_string()))
            }
        }
    }

    /// Convert a SILK audio file to WAV using ffmpeg.
    ///
    /// Output: 16kHz mono PCM WAV (suitable for Whisper transcription).
    async fn convert_silk_to_wav(
        &self,
        silk_path: &std::path::Path,
    ) -> anyhow::Result<String> {
        // Discover ffmpeg binary
        let ffmpeg_path = which::which("ffmpeg")
            .map_err(|e| anyhow::anyhow!("ffmpeg not found: {}", e))?;

        let wav_path = silk_path.with_extension("wav");

        let output = tokio::process::Command::new(&ffmpeg_path)
            .args([
                "-y",                    // overwrite output
                "-i",
                silk_path.to_str().unwrap(),
                "-acodec",
                "pcm_s16le",           // 16-bit signed little-endian PCM
                "-ar",
                "16000",                // 16kHz (Whisper optimal)
                "-ac",
                "1",                    // mono
                wav_path.to_str().unwrap(),
            ])
            .output()
            .await
            .with_context(|| "run ffmpeg")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ffmpeg SILK→WAV failed: {}", stderr);
        }

        tracing::debug!("WeXin: converted {} → {}", silk_path.display(), wav_path.display());
        Ok(wav_path.to_string_lossy().to_string())
    }

    /// Upload a WAV/MP3/OGG file as a voice message.
    ///
    /// Flow:
    /// 1. If not SILK format: ffmpeg encode WAV → SILK
    /// 2. AES-128-ECB encrypt SILK data
    /// 3. getUploadUrl → PUT to CDN
    /// 4. Return (encrypt_query_param, aes_key)
    pub async fn upload_voice(
        &self,
        to_user_id: &str,
        file_path: &std::path::Path,
    ) -> anyhow::Result<(String, String)> {
        // Step 1: Convert to SILK if needed
        let silk_path = if file_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("silk"))
            .unwrap_or(false)
        {
            file_path.to_path_buf()
        } else {
            self.encode_to_silk(file_path).await?
        };

        // Step 2: Read SILK data
        let data = tokio::fs::read(&silk_path).await
            .with_context(|| format!("read voice file {:?}", silk_path))?;
        let raw_size = data.len() as u64;
        let raw_md5 = self::crypto::md5_hex(&data);

        // Step 3: AES-128-ECB encrypt
        let key = self::crypto::AesKey::random();
        let aes_key_b64 = key.to_base64_string();
        let encrypted = self::crypto::encrypt(key.as_bytes(), &data)
            .context("AES encrypt voice")?;
        let encrypted_size = encrypted.len() as u64;

        // Step 4: getUploadUrl
        let params = self::api::UploadParams {
            filekey: Uuid::new_v4().to_string(),
            media_type: 3, // VOICE
            to_user_id: to_user_id.to_string(),
            rawsize: raw_size,
            rawfilemd5: raw_md5,
            filesize: encrypted_size,
            thumb_rawsize: None,
            thumb_rawfilemd5: None,
            thumb_filesize: None,
        };

        let upload_resp = self
            .api
            .get_upload_url(&params)
            .await
            .context("iLink getUploadUrl for voice")?;

        // Step 5: PUT to CDN
        let put_resp = self
            .api
            .http_client()
            .put(&upload_resp.upload_param)
            .header("Content-Type", "application/octet-stream")
            .body(encrypted)
            .send()
            .await
            .context("CDN PUT for voice")?;

        if !put_resp.status().is_success() {
            let err = put_resp.text().await.unwrap_or_default();
            anyhow::bail!("CDN voice upload failed: {}", err);
        }

        // Clean up temp SILK file if we created one
        if silk_path != file_path {
            let _ = tokio::fs::remove_file(&silk_path).await;
        }

        tracing::debug!(
            "WeXin: uploaded voice {} bytes ({} → CDN)",
            data.len(),
            encrypted.len()
        );

        Ok((upload_resp.upload_param, aes_key_b64))
    }

    /// Encode any audio format to SILK using ffmpeg.
    async fn encode_to_silk(
        &self,
        input_path: &std::path::Path,
    ) -> anyhow::Result<std::path::PathBuf> {
        let ffmpeg_path = which::which("ffmpeg")
            .map_err(|_| anyhow::anyhow!("ffmpeg not found"))?;

        let silk_path = input_path.with_extension("silk");

        // ffmpeg can output to Silk format via libopus or native Silk encoder
        // We use a two-step: any format → WAV (16kHz mono) → SILK
        // For simplicity, convert to a known intermediate format first
        let pcm_path = input_path.with_extension("pcm");

        // Step 1: any → 16kHz mono PCM
        let pcm_out = tokio::process::Command::new(&ffmpeg_path)
            .args([
                "-y",
                "-i",
                input_path.to_str().unwrap(),
                "-acodec",
                "pcm_s16le",
                "-ar",
                "16000",
                "-ac",
                "1",
                pcm_path.to_str().unwrap(),
            ])
            .output()
            .await
            .with_context(|| "ffmpeg convert to PCM")?;

        if !pcm_out.status.success() {
            let stderr = String::from_utf8_lossy(&pcm_out.stderr);
            anyhow::bail!("ffmpeg PCM conversion failed: {}", stderr);
        }

        // Step 2: PCM → SILK (ffmpeg native Silk encoder)
        let silk_out = tokio::process::Command::new(&ffmpeg_path)
            .args([
                "-y",
                "-i",
                pcm_path.to_str().unwrap(),
                "-c:a",
                "libopus",  // or "silk" if ffmpeg was built with Silk support
                "-ar",
                "16000",
                "-ac",
                "1",
                silk_path.to_str().unwrap(),
            ])
            .output()
            .await
            .with_context(|| "ffmpeg encode to SILK")?;

        // Clean up PCM intermediate
        let _ = tokio::fs::remove_file(&pcm_path).await;

        if !silk_out.status.success() {
            let stderr = String::from_utf8_lossy(&silk_out.stderr);
            // Fallback: just return the original file (will try to upload as-is)
            tracing::warn!("ffmpeg SILK encode failed: {}", stderr);
            return Ok(input_path.to_path_buf());
        }

        tracing::debug!("WeXin: encoded {} → SILK", input_path.display());
        Ok(silk_path)
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Phase 3 — File & Video Messages
// ═════════════════════════════════════════════════════════════════════════════

impl WeXinChannel {
    /// Download a generic file from CDN (file_message or video).
    ///
    /// Unlike images, WeChat files don't always include a filename.
    /// The extension is inferred from the CDN URL or defaulted to "bin".
    async fn download_generic_media(
        &self,
        encrypt_param: &str,
        aes_key_b64: &str,
        suggested_ext: &str,
    ) -> anyhow::Result<(String, Option<String>)> {
        // Step 1: Decrypt CDN URL
        let url = self::crypto::decrypt_cdn_url(encrypt_param, aes_key_b64)
            .await
            .context("decrypt CDN URL for file")?;

        // Step 2: Download encrypted blob
        let encrypted = self
            .api
            .http_client()
            .get(&url)
            .send()
            .await
            .with_context(|| "download file from CDN")?
            .bytes()
            .await
            .with_context(|| "read file bytes")?
            .to_vec();

        // Step 3: Size check (cap at 100 MB for files)
        const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;
        if encrypted.len() as u64 > MAX_FILE_SIZE {
            anyhow::bail!(
                "WeXin: file too large ({} bytes, max {} bytes)",
                encrypted.len(),
                MAX_FILE_SIZE
            );
        }

        // Step 4: AES-128-ECB decrypt
        let key = AesKey::from_base64(aes_key_b64)
            .context("parse file AES key")?;
        let decrypted = self::crypto::decrypt(key.as_bytes(), &encrypted)
            .context("AES decrypt file")?;

        // Step 5: Detect MIME from magic bytes (or use suggested extension)
        let mime = self::crypto::detect_mime_from_magic(&decrypted);

        // Step 6: Infer extension from URL if possible, else from MIME
        let ext = std::path::Path::new(&url)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_else(|| self::crypto::mime_to_ext(mime).to_string());

        // Step 7: Save to temp file
        let temp_dir = std::env::temp_dir();
        let filename = format!("weixin_file_{}.{}", Uuid::new_v4(), ext);
        let path = temp_dir.join(&filename);
        tokio::fs::write(&path, &decrypted)
            .await
            .with_context(|| format!("write temp file to {}", path.display()))?;

        tracing::debug!(
            "WeXin: downloaded file {} bytes as {} ({})",
            decrypted.len(),
            path.display(),
            mime
        );

        Ok((path.to_string_lossy().to_string(), Some(mime.to_string())))
    }

    /// Download a video from CDN (with optional thumbnail).
    async fn download_video(
        &self,
        video_param: &str,
        video_key_b64: &str,
        thumb_param: Option<&str>,
        thumb_key_b64: Option<&str>,
    ) -> anyhow::Result<(String, Option<String>)> {
        // Download video
        let (video_path, _) = self
            .download_generic_media(video_param, video_key_b64, "mp4")
            .await
            .context("download video")?;

        // Optionally download thumbnail
        let thumb_path = match (thumb_param, thumb_key_b64) {
            (Some(p), Some(k)) => {
                match self.download_generic_media(p, k, "jpg").await {
                    Ok((path, _)) => Some(path),
                    Err(e) => {
                        tracing::warn!("WeXin: thumbnail download failed: {}", e);
                        None
                    }
                }
            }
            _ => None,
        };

        tracing::debug!(
            "WeXin: downloaded video {} (thumb: {:?})",
            video_path,
            thumb_path
        );

        Ok((video_path, thumb_path))
    }

    /// Upload a generic file to CDN. Returns (encrypt_query_param, aes_key).
    pub async fn upload_file(
        &self,
        to_user_id: &str,
        file_path: &std::path::Path,
    ) -> anyhow::Result<(String, String)> {
        // Step 1: Read file
        let data = tokio::fs::read(file_path).await
            .with_context(|| format!("read file {:?}", file_path))?;
        let raw_size = data.len() as u64;
        let raw_md5 = self::crypto::md5_hex(&data);

        // Step 2: Detect MIME for media_type
        let mime = self::crypto::detect_mime_from_magic(&data);
        let media_type = match mime {
            "video/mp4" | "video/avi" | "video/quicktime" | "video/x-matroska" => 2u8,
            _ => 4u8, // Generic FILE
        };

        // Step 3: AES-128-ECB encrypt
        let key = self::crypto::AesKey::random();
        let aes_key_b64 = key.to_base64_string();
        let encrypted = self::crypto::encrypt(key.as_bytes(), &data)
            .context("AES encrypt file")?;
        let encrypted_size = encrypted.len() as u64;

        // Step 4: getUploadUrl
        let params = self::api::UploadParams {
            filekey: Uuid::new_v4().to_string(),
            media_type,
            to_user_id: to_user_id.to_string(),
            rawsize: raw_size,
            rawfilemd5: raw_md5,
            filesize: encrypted_size,
            thumb_rawsize: None,
            thumb_rawfilemd5: None,
            thumb_filesize: None,
        };

        let upload_resp = self
            .api
            .get_upload_url(&params)
            .await
            .context("iLink getUploadUrl for file")?;

        // Step 5: PUT to CDN
        let put_resp = self
            .api
            .http_client()
            .put(&upload_resp.upload_param)
            .header("Content-Type", "application/octet-stream")
            .body(encrypted)
            .send()
            .await
            .context("CDN PUT for file")?;

        if !put_resp.status().is_success() {
            let err = put_resp.text().await.unwrap_or_default();
            anyhow::bail!("CDN file upload failed: {}", err);
        }

        tracing::debug!(
            "WeXin: uploaded file {} ({} bytes, MIME={})",
            file_path.display(),
            data.len(),
            mime
        );

        Ok((upload_resp.upload_param, aes_key_b64))
    }

    /// Upload a video file with thumbnail.
    ///
    /// If ffmpeg is available, extracts a thumbnail automatically.
    pub async fn upload_video(
        &self,
        to_user_id: &str,
        file_path: &std::path::Path,
    ) -> anyhow::Result<(String, String, Option<(String, String)>)> {
        // Step 1: Read video file
        let data = tokio::fs::read(file_path).await
            .with_context(|| format!("read video {:?}", file_path))?;
        let raw_size = data.len() as u64;
        let raw_md5 = self::crypto::md5_hex(&data);

        // Step 2: Generate thumbnail
        let thumb_result = self.generate_video_thumbnail(file_path).await;

        // Step 3: AES-128-ECB encrypt video
        let key = self::crypto::AesKey::random();
        let aes_key_b64 = key.to_base64_string();
        let encrypted = self::crypto::encrypt(key.as_bytes(), &data)
            .context("AES encrypt video")?;
        let encrypted_size = encrypted.len() as u64;

        // Step 4: Encrypt thumbnail if available
        let (thumb_encrypted, thumb_params_result) = match thumb_result {
            Ok((thumb_data, thumb_md5)) => {
                let thumb_enc = self::crypto::encrypt(key.as_bytes(), &thumb_data)?;
                let thumb_size = thumb_enc.len() as u64;
                let thumb_params = (
                    thumb_enc,
                    thumb_md5.clone(),
                    thumb_size,
                    thumb_md5,
                );
                (Some(thumb_params.0.clone()), Some(thumb_params))
            }
            Err(e) => {
                tracing::warn!("WeXin: thumbnail generation failed: {}", e);
                (None, None)
            }
        };

        // Step 5: getUploadUrl (video)
        let params = self::api::UploadParams {
            filekey: Uuid::new_v4().to_string(),
            media_type: 2, // VIDEO
            to_user_id: to_user_id.to_string(),
            rawsize: raw_size,
            rawfilemd5: raw_md5,
            filesize: encrypted_size,
            thumb_rawsize: thumb_result.ok().map(|(d, _)| d.len() as u64),
            thumb_rawfilemd5: thumb_result.ok().map(|(_, m)| m.clone()),
            thumb_filesize: thumb_encrypted.as_ref().map(|d| d.len() as u64),
        };

        let upload_resp = self
            .api
            .get_upload_url(&params)
            .await
            .context("iLink getUploadUrl for video")?;

        // Step 6: PUT video to CDN
        let put_resp = self
            .api
            .http_client()
            .put(&upload_resp.upload_param)
            .header("Content-Type", "application/octet-stream")
            .body(encrypted)
            .send()
            .await
            .context("CDN PUT for video")?;

        if !put_resp.status().is_success() {
            let err = put_resp.text().await.unwrap_or_default();
            anyhow::bail!("CDN video upload failed: {}", err);
        }

        // Step 7: PUT thumbnail to CDN (separate CDN path, separate key)
        let thumb_cdn_result = match thumb_encrypted {
            Some(thumb_data) => {
                let thumb_key = self::crypto::AesKey::random();
                let thumb_key_b64 = thumb_key.to_base64_string();
                let thumb_encrypted_final = self::crypto::encrypt(
                    thumb_key.as_bytes(),
                    &thumb_data,
                )?;
                let thumb_upload_resp = self
                    .api
                    .get_upload_url(&self::api::UploadParams {
                        filekey: Uuid::new_v4().to_string(),
                        media_type: 1, // IMAGE
                        to_user_id: to_user_id.to_string(),
                        rawsize: thumb_data.len() as u64,
                        rawfilemd5: thumb_result.as_ref().ok().map(|(_, m)| m.clone()).unwrap_or_default(),
                        filesize: thumb_encrypted_final.len() as u64,
                        thumb_rawsize: None,
                        thumb_rawfilemd5: None,
                        thumb_filesize: None,
                    })
                    .await;

                match thumb_upload_resp {
                    Ok(tu) => {
                        let thumb_put = self
                            .api
                            .http_client()
                            .put(&tu.upload_param)
                            .header("Content-Type", "application/octet-stream")
                            .body(thumb_encrypted_final)
                            .send()
                            .await;
                        if thumb_put.is_ok() && thumb_put.as_ref().unwrap().status().is_success() {
                            Some((tu.upload_param, thumb_key_b64))
                        } else {
                            tracing::warn!("WeXin: thumbnail CDN PUT failed");
                            None
                        }
                    }
                    Err(e) => {
                        tracing::warn!("WeXin: thumbnail getUploadUrl failed: {}", e);
                        None
                    }
                }
            }
            None => None,
        };

        tracing::debug!(
            "WeXin: uploaded video {} ({} bytes)",
            file_path.display(),
            data.len()
        );

        Ok((upload_resp.upload_param, aes_key_b64, thumb_cdn_result))
    }

    /// Extract a thumbnail from a video using ffmpeg.
    /// Returns (thumbnail_bytes, md5_hex) at 2 seconds into the video.
    async fn generate_video_thumbnail(
        &self,
        video_path: &std::path::Path,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        let ffmpeg_path = which::which("ffmpeg")
            .map_err(|_| anyhow::anyhow!("ffmpeg not found"))?;

        let thumb_path = std::env::temp_dir()
            .join(format!("weixin_thumb_{}.jpg", Uuid::new_v4()));

        let output = tokio::process::Command::new(&ffmpeg_path)
            .args([
                "-y",
                "-ss",
                "00:00:02",         // seek to 2 seconds
                "-i",
                video_path.to_str().unwrap(),
                "-vframes",
                "1",                // extract 1 frame
                "-q:v",
                "2",               // quality (lower = better)
                "-s",
                "200x200",         // resize to 200x200
                thumb_path.to_str().unwrap(),
            ])
            .output()
            .await
            .with_context(|| "run ffmpeg for thumbnail")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ffmpeg thumbnail extraction failed: {}", stderr);
        }

        let thumb_data = tokio::fs::read(&thumb_path).await
            .with_context(|| "read thumbnail file")?;
        let md5 = self::crypto::md5_hex(&thumb_data);

        // Clean up temp thumbnail
        let _ = tokio::fs::remove_file(&thumb_path).await;

        Ok((thumb_data, md5))
    }
}
