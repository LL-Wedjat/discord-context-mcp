use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use futures_util::{FutureExt, SinkExt, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT as USER_AGENT_HEADER};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::db::Db;
use crate::models::{
    DiscordChannel, DiscordGuild, DiscordMessage, DiscordUser, DmChannel, PartialDiscordMessage,
};

const DEFAULT_BASE_URL: &str = "https://discord.com/api/v10";
const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";
pub const USER_AGENT: &str = concat!(
    "discord-context-mcp/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/LL-Wedjat/discord-context-mcp)"
);

#[derive(thiserror::Error, Debug)]
pub enum WriteApiError {
    #[error("Write burst limit exceeded (max 5 actions per 60s). Retry after {retry_after_secs}s")]
    BurstLimitExceeded { retry_after_secs: u64 },
    #[error("Discord write rate limited. Retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("Discord CAPTCHA or challenge detected: {0}")]
    CaptchaOrChallenge(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bucket {
    pub remaining: i64,
    pub reset_at_ms: i64,
}

pub fn compute_bucket_wait_ms(bucket: &Bucket, now_ms: i64) -> u64 {
    if bucket.remaining <= 0 && bucket.reset_at_ms > now_ms {
        (bucket.reset_at_ms - now_ms) as u64
    } else {
        0
    }
}

/// Compute how long to wait after an HTTP 429 response, in milliseconds.
///
/// Prefers `retry_after` (seconds), then falls back to `reset_after`
/// (milliseconds), then to 0ms. A 1000ms safety buffer is added so we never
/// retry exactly at the reset boundary, and the result is capped at 60s.
pub fn rate_limit_delay_ms(retry_after_s: Option<f64>, reset_after_ms: Option<i64>) -> u64 {
    let base_ms = retry_after_s
        .map(|s| (s * 1000.0) as i64)
        .or(reset_after_ms)
        .unwrap_or(0)
        .max(0) as u64;
    base_ms.saturating_add(1000).min(60_000)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SyncReport {
    pub channels_fetched: usize,
    pub messages_fetched: usize,
    pub errors: Vec<String>,
}

pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < input.len() {
        let b0 = input[i] as usize;
        let b1 = if i + 1 < input.len() {
            input[i + 1] as usize
        } else {
            0
        };
        let b2 = if i + 2 < input.len() {
            input[i + 2] as usize
        } else {
            0
        };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[(triple >> 18) & 0x3F] as char);
        out.push(TABLE[(triple >> 12) & 0x3F] as char);
        if i + 1 < input.len() {
            out.push(TABLE[(triple >> 6) & 0x3F] as char);
        } else {
            out.push('=');
        }
        if i + 2 < input.len() {
            out.push(TABLE[triple & 0x3F] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

pub fn make_super_properties() -> String {
    let props = json!({
        "os": "Windows",
        "browser": "Chrome",
        "device": "",
        "system_locale": "en-US",
        "browser_user_agent": BROWSER_USER_AGENT,
        "browser_version": "133.0.0.0",
        "os_version": "10",
        "referrer": "",
        "referring_domain": "",
        "referrer_current": "",
        "referring_domain_current": "",
        "release_channel": "stable",
        "client_build_number": 367200,
        "client_event_source": null
    });
    base64_encode(props.to_string().as_bytes())
}

/// Builds the default headers used for all REST API calls.
///
/// REST requests identify the client honestly and do not send browser
/// client spoofing headers. The gateway still uses `make_super_properties`
/// for its IDENTIFY payload; that is intentionally left unchanged.
pub fn build_rest_headers() -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT_HEADER, HeaderValue::from_static(USER_AGENT));
    Ok(headers)
}

pub fn normalize_route_key(method: &reqwest::Method, path: &str) -> String {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let mut norm_parts = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        if i > 0 && parts[i - 1] == "reactions" && *part != "@me" {
            norm_parts.push(":emoji");
        } else if !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()) {
            norm_parts.push(":id");
        } else {
            norm_parts.push(*part);
        }
    }
    format!("{}:/{}", method.as_str(), norm_parts.join("/"))
}

pub fn encode_path_segment(s: &str) -> Result<String> {
    if s.is_empty() || s.trim().is_empty() {
        return Err(anyhow!("Path segment cannot be empty"));
    }
    if s == "." {
        return Err(anyhow!("Path segment cannot be '.'"));
    }
    if s.contains('/') || s.contains('?') || s.contains('#') || s.contains("..") {
        return Err(anyhow!("Path segment contains invalid characters (/ ? # ..)"));
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(anyhow!("Path segment contains whitespace or control characters"));
    }
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match *b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => {
                out.push(*b as char);
            }
            byte => {
                use std::fmt::Write;
                let _ = write!(out, "%{:02X}", byte);
            }
        }
    }
    Ok(out)
}

pub fn parse_search_response(value: &serde_json::Value) -> Result<Vec<DiscordMessage>> {
    let mut messages = Vec::new();
    let mut seen_ids = HashSet::new();
    let groups = match value.get("messages").and_then(|m| m.as_array()) {
        Some(groups) => groups,
        None => return Ok(messages),
    };
    for group in groups {
        let items = match group.as_array() {
            Some(items) => items,
            None => continue,
        };
        for item in items {
            let msg: DiscordMessage = serde_json::from_value(item.clone())
                .context("Failed to deserialize search result message")?;
            if seen_ids.insert(msg.id.clone()) {
                messages.push(msg);
            }
        }
    }
    Ok(messages)
}

#[derive(Clone)]
pub struct DiscordClient {
    pub token: String,
    pub http: reqwest::Client,
    pub db: Arc<Db>,
    pub base: String,
    pub rate_limits: Arc<Mutex<HashMap<String, Bucket>>>,
    pub global_reset_ms: Arc<AtomicI64>,
    pub me: Arc<Mutex<Option<DiscordUser>>>,
    pub shutdown: Arc<AtomicBool>,
    /// Wakes the gateway task immediately on `stop()`. The `AtomicBool` stays
    /// as the authoritative state; this only removes the wakeup latency.
    pub shutdown_notify: Arc<tokio::sync::Notify>,
    pub last_identify: Arc<Mutex<Instant>>,
    pub rest_semaphore: Arc<tokio::sync::Semaphore>,
    pub write_timestamps: Arc<std::sync::Mutex<VecDeque<Instant>>>,
    pub write_nonce_seq: Arc<AtomicU64>,
}

impl DiscordClient {
    pub fn new(token: String, db: Arc<Db>) -> Result<Self> {
        let default_headers = build_rest_headers()?;

        let http = reqwest::Client::builder()
            .default_headers(default_headers)
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to construct reqwest HTTP client")?;

        Ok(DiscordClient {
            token,
            http,
            db,
            base: DEFAULT_BASE_URL.to_string(),
            rate_limits: Arc::new(Mutex::new(HashMap::new())),
            global_reset_ms: Arc::new(AtomicI64::new(0)),
            me: Arc::new(Mutex::new(None)),
            shutdown: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(tokio::sync::Notify::new()),
            last_identify: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(10))),
            rest_semaphore: Arc::new(tokio::sync::Semaphore::new(2)),
            write_timestamps: Arc::new(std::sync::Mutex::new(VecDeque::new())),
            write_nonce_seq: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn sanitize(&self, msg: &str) -> String {
        if self.token.is_empty() {
            msg.to_string()
        } else {
            msg.replace(&self.token, "[REDACTED]")
        }
    }

    pub async fn init(&self) -> Result<DiscordUser> {
        if self.token.trim().is_empty() {
            return Err(anyhow!("Discord token missing"));
        }

        let resp_val = self
            .rest(reqwest::Method::GET, "/users/@me", None, None)
            .await?;

        let val = resp_val.ok_or_else(|| anyhow!("Empty response from /users/@me"))?;
        let user: DiscordUser = serde_json::from_value(val)
            .context("Failed to deserialize /users/@me into DiscordUser")?;

        if self
            .db
            .ensure_account(&user.id)
            .context("Failed to verify cached account in database")?
        {
            info!(
                "Cached account changed to {}; cleared stale cache from the previous account",
                user.id
            );
        }

        self.db
            .upsert_user(&user)
            .context("Failed to cache @me user in database")?;

        let mut me_lock = self.me.lock().await;
        *me_lock = Some(user.clone());

        Ok(user)
    }

    pub async fn ensure_me(&self) -> Result<String> {
        {
            let me_guard = self.me.lock().await;
            if let Some(user) = me_guard.as_ref() {
                return Ok(user.id.clone());
            }
        }
        let user = self.init().await?;
        Ok(user.id)
    }

    pub async fn rest(
        &self,
        method: reqwest::Method,
        path: &str,
        query: Option<&[(String, String)]>,
        body: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        let route_key = normalize_route_key(&method, path);
        let url_str = format!("{}{}", self.base.trim_end_matches('/'), path);

        let mut attempts = 0;
        const MAX_ATTEMPTS: usize = 5;

        loop {
            let permit = self
                .rest_semaphore
                .acquire()
                .await
                .map_err(|e| anyhow!("REST semaphore closed: {}", e))?;

            let now_ms = chrono::Utc::now().timestamp_millis();
            let global_reset = self.global_reset_ms.load(Ordering::SeqCst);
            if global_reset > now_ms {
                let sleep_ms = (global_reset - now_ms) as u64;
                debug!("Global rate limit active, waiting {}ms", sleep_ms);
                drop(permit);
                tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                continue;
            }

            {
                let rl = self.rate_limits.lock().await;
                let wait_ms = if let Some(bucket) = rl.get(&route_key) {
                    compute_bucket_wait_ms(bucket, chrono::Utc::now().timestamp_millis())
                } else {
                    0
                };
                drop(rl);

                if wait_ms > 0 {
                    debug!(
                        "Route rate limit for {} exceeded, waiting {}ms",
                        route_key, wait_ms
                    );
                    drop(permit);
                    tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                    continue;
                }
            }

            attempts += 1;

            let mut req_builder = self
                .http
                .request(method.clone(), &url_str)
                .header(AUTHORIZATION, &self.token);

            if let Some(q) = query {
                req_builder = req_builder.query(q);
            }
            if let Some(b) = body {
                req_builder = req_builder.json(b);
            }

            let response = req_builder.send().await;
            let resp = match response {
                Ok(r) => r,
                Err(e) => {
                    let sanitized = self.sanitize(&e.to_string());
                    return Err(anyhow!("HTTP request failed: {}", sanitized));
                }
            };

            let status = resp.status();
            let headers = resp.headers().clone();

            let rem_header = headers
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok());
            let reset_after_header = headers
                .get("x-ratelimit-reset-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| (v * 1000.0) as i64);

            let is_global = headers
                .get("x-ratelimit-global")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

            if let (Some(rem), Some(reset_ms)) = (rem_header, reset_after_header) {
                let target_ms = chrono::Utc::now().timestamp_millis() + reset_ms;
                if is_global {
                    self.global_reset_ms.store(target_ms, Ordering::SeqCst);
                } else {
                    let mut rl = self.rate_limits.lock().await;
                    rl.insert(
                        route_key.clone(),
                        Bucket {
                            remaining: rem,
                            reset_at_ms: target_ms,
                        },
                    );
                }
            }

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let body_text = resp.text().await.unwrap_or_default();
                let retry_after_s =
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body_text) {
                        val.get("retry_after").and_then(|v| v.as_f64())
                    } else {
                        None
                    };

                let sleep_ms = rate_limit_delay_ms(retry_after_s, reset_after_header);

                let target_reset_ms = chrono::Utc::now().timestamp_millis() + sleep_ms as i64;
                if is_global {
                    self.global_reset_ms
                        .store(target_reset_ms, Ordering::SeqCst);
                } else {
                    let mut rl = self.rate_limits.lock().await;
                    rl.insert(
                        route_key.clone(),
                        Bucket {
                            remaining: 0,
                            reset_at_ms: target_reset_ms,
                        },
                    );
                }

                warn!(
                    "Rate limited (429) on {}, sleeping {}ms (attempt {}/{})",
                    route_key, sleep_ms, attempts, MAX_ATTEMPTS
                );

                if attempts >= MAX_ATTEMPTS {
                    return Err(anyhow!(
                        "Exceeded max retry attempts (429) for {}",
                        route_key
                    ));
                }

                drop(permit);
                tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                continue;
            }

            if status == reqwest::StatusCode::NO_CONTENT {
                return Ok(None);
            }

            if !status.is_success() {
                let err_text = resp.text().await.unwrap_or_default();
                let sanitized_body = self.sanitize(&err_text);
                return Err(anyhow!(
                    "Discord API error HTTP {}: {}",
                    status.as_u16(),
                    sanitized_body
                ));
            }

            let val: serde_json::Value = resp.json().await.map_err(|e| {
                anyhow!(
                    "Failed to parse response JSON: {}",
                    self.sanitize(&e.to_string())
                )
            })?;

            return Ok(Some(val));
        }
    }

    pub async fn rest_write(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        let route_key = normalize_route_key(&method, path);

        // 1. Fail-fast rate limit check (do NOT sleep, do NOT consume burst-cap slot)
        let now_ms = chrono::Utc::now().timestamp_millis();
        let global_reset = self.global_reset_ms.load(Ordering::SeqCst);
        if global_reset > now_ms {
            let retry_after_ms = (global_reset - now_ms) as u64;
            return Err(WriteApiError::RateLimited { retry_after_ms }.into());
        }

        {
            let rl = self.rate_limits.lock().await;
            let wait_ms = if let Some(bucket) = rl.get(&route_key) {
                compute_bucket_wait_ms(bucket, now_ms)
            } else {
                0
            };
            drop(rl);

            if wait_ms > 0 {
                return Err(WriteApiError::RateLimited {
                    retry_after_ms: wait_ms,
                }
                .into());
            }
        }

        // 2. Burst cap: max 5 actions per 60s sliding window
        {
            let now = Instant::now();
            let sixty_secs = Duration::from_secs(60);
            let mut timestamps = self
                .write_timestamps
                .lock()
                .map_err(|e| anyhow!("Failed to lock write_timestamps: {}", e))?;

            while let Some(&front) = timestamps.front() {
                if now.duration_since(front) >= sixty_secs {
                    timestamps.pop_front();
                } else {
                    break;
                }
            }

            if timestamps.len() >= 5 {
                let oldest = *timestamps.front().unwrap();
                let elapsed = now.duration_since(oldest);
                let retry_after_secs = sixty_secs
                    .saturating_sub(elapsed)
                    .as_secs_f64()
                    .ceil()
                    .max(1.0) as u64;
                return Err(WriteApiError::BurstLimitExceeded { retry_after_secs }.into());
            }

            timestamps.push_back(now);
        } // lock dropped before any .await

        // 3. Single HTTP request using rest_semaphore
        let permit = self
            .rest_semaphore
            .acquire()
            .await
            .map_err(|e| anyhow!("REST semaphore closed: {}", e))?;

        let url_str = format!("{}{}", self.base.trim_end_matches('/'), path);
        let mut req_builder = self
            .http
            .request(method.clone(), &url_str)
            .header(AUTHORIZATION, &self.token);

        if let Some(b) = body {
            req_builder = req_builder.json(b);
        }

        let response = req_builder.send().await;
        drop(permit);

        let resp = match response {
            Ok(r) => r,
            Err(e) => {
                let sanitized = self.sanitize(&e.to_string());
                return Err(anyhow!("HTTP write request failed: {}", sanitized));
            }
        };

        let status = resp.status();
        let headers = resp.headers().clone();

        let rem_header = headers
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok());
        let reset_after_header = headers
            .get("x-ratelimit-reset-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| (v * 1000.0) as i64);
        let is_global = headers
            .get("x-ratelimit-global")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        if let (Some(rem), Some(reset_ms)) = (rem_header, reset_after_header) {
            let target_ms = chrono::Utc::now().timestamp_millis() + reset_ms;
            if is_global {
                self.global_reset_ms.store(target_ms, Ordering::SeqCst);
            } else {
                let mut rl = self.rate_limits.lock().await;
                rl.insert(
                    route_key.clone(),
                    Bucket {
                        remaining: rem,
                        reset_at_ms: target_ms,
                    },
                );
            }
        }

        // 4. Status 204 No Content
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }

        // 5. Any non-2xx status: check for CAPTCHA or Cloudflare challenge
        if !status.is_success() {
            let raw_body = resp.text().await.unwrap_or_default();
            let sanitized_body = self.sanitize(&raw_body);
            let lower_body = raw_body.to_ascii_lowercase();

            if lower_body.contains("captcha_key")
                || lower_body.contains("<html")
                || lower_body.contains("<!doctype html")
                || lower_body.contains("cf-browser-verification")
            {
                return Err(WriteApiError::CaptchaOrChallenge(sanitized_body).into());
            }

            // 6. Non-captcha 429: update rate limit bucket and return RateLimited
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after_s =
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw_body) {
                        val.get("retry_after").and_then(|v| v.as_f64())
                    } else {
                        None
                    };
                let sleep_ms = rate_limit_delay_ms(retry_after_s, reset_after_header);
                let target_reset_ms = chrono::Utc::now().timestamp_millis() + sleep_ms as i64;

                if is_global {
                    self.global_reset_ms
                        .store(target_reset_ms, Ordering::SeqCst);
                } else {
                    let mut rl = self.rate_limits.lock().await;
                    rl.insert(
                        route_key.clone(),
                        Bucket {
                            remaining: 0,
                            reset_at_ms: target_reset_ms,
                        },
                    );
                }
                return Err(WriteApiError::RateLimited {
                    retry_after_ms: sleep_ms,
                }
                .into());
            }

            // 7. Other non-2xx error
            return Err(anyhow!(
                "Discord API error HTTP {}: {}",
                status.as_u16(),
                sanitized_body
            ));
        }

        // Success response with body
        let val: serde_json::Value = resp.json().await.map_err(|e| {
            anyhow!(
                "Failed to parse write response JSON: {}",
                self.sanitize(&e.to_string())
            )
        })?;

        Ok(Some(val))
    }

    pub async fn fetch_message_around(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> Result<Option<DiscordMessage>> {
        let path = format!("/channels/{}/messages", channel_id);
        let query = vec![
            ("around".to_string(), message_id.to_string()),
            ("limit".to_string(), "5".to_string()),
        ];

        let resp = self
            .rest(reqwest::Method::GET, &path, Some(&query), None)
            .await?;

        let val = resp.ok_or_else(|| anyhow!("Empty messages response"))?;
        let messages: Vec<DiscordMessage> =
            serde_json::from_value(val).context("Failed to deserialize messages array")?;

        let mut target = None;
        for msg in &messages {
            let _ = self.db.upsert_message(msg);
            if msg.id == message_id {
                target = Some(msg.clone());
            }
        }

        Ok(target)
    }

    pub async fn send_message(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
        mention_reply_author: bool,
    ) -> Result<DiscordMessage> {
        let seq = self.write_nonce_seq.fetch_add(1, Ordering::SeqCst);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let nonce = format!("{}_{}", now_ms, seq);

        let mut body = json!({
            "content": content,
            "nonce": nonce,
            "enforce_nonce": true,
        });

        let mut allowed_mentions = json!({
            "parse": ["users"]
        });

        if let Some(ref_id) = reply_to {
            body["message_reference"] = json!({
                "message_id": ref_id,
                "fail_if_not_exists": false
            });
            allowed_mentions["replied_user"] = json!(mention_reply_author);
        }

        body["allowed_mentions"] = allowed_mentions;

        let path = format!("/channels/{}/messages", channel_id);
        let resp = self
            .rest_write(reqwest::Method::POST, &path, Some(&body))
            .await?;

        let val = resp.ok_or_else(|| anyhow!("Empty response from send_message"))?;
        let message: DiscordMessage = serde_json::from_value(val)
            .context("Failed to deserialize sent message response")?;
        let _ = self.db.upsert_message(&message);

        Ok(message)
    }

    pub async fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<DiscordMessage> {
        let body = json!({
            "content": content,
            "allowed_mentions": {
                "parse": ["users"]
            }
        });

        let path = format!("/channels/{}/messages/{}", channel_id, message_id);
        let resp = self
            .rest_write(reqwest::Method::PATCH, &path, Some(&body))
            .await?;

        let val = resp.ok_or_else(|| anyhow!("Empty response from edit_message"))?;
        let message: DiscordMessage = serde_json::from_value(val)
            .context("Failed to deserialize edited message response")?;
        let _ = self.db.upsert_message(&message);

        Ok(message)
    }

    pub async fn delete_message(&self, channel_id: &str, message_id: &str) -> Result<()> {
        let path = format!("/channels/{}/messages/{}", channel_id, message_id);
        self.rest_write(reqwest::Method::DELETE, &path, None)
            .await?;
        let _ = self.db.delete_message(message_id);
        Ok(())
    }

    pub async fn add_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        let encoded = encode_path_segment(emoji)?;
        let path = format!(
            "/channels/{}/messages/{}/reactions/{}/@me",
            channel_id, message_id, encoded
        );
        self.rest_write(reqwest::Method::PUT, &path, None).await?;
        Ok(())
    }

    pub async fn remove_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        let encoded = encode_path_segment(emoji)?;
        let path = format!(
            "/channels/{}/messages/{}/reactions/{}/@me",
            channel_id, message_id, encoded
        );
        self.rest_write(reqwest::Method::DELETE, &path, None)
            .await?;
        Ok(())
    }

    pub async fn list_guilds(&self) -> Result<Vec<DiscordGuild>> {
        let resp = self
            .rest(reqwest::Method::GET, "/users/@me/guilds", None, None)
            .await?;

        let val = resp.ok_or_else(|| anyhow!("Empty guilds response"))?;
        let guilds: Vec<DiscordGuild> =
            serde_json::from_value(val).context("Failed to deserialize guilds array")?;

        let mut ids = Vec::with_capacity(guilds.len());
        for g in &guilds {
            ids.push(g.id.clone());
            let _ = self.db.upsert_guild(g);
        }

        if !ids.is_empty() {
            let _ = self.db.prune_guilds_not_in(&ids);
        }
        Ok(guilds)
    }

    pub async fn list_channels(&self, guild_id: &str) -> Result<Vec<DiscordChannel>> {
        let path = format!("/guilds/{}/channels", guild_id);
        let resp = self.rest(reqwest::Method::GET, &path, None, None).await?;

        let val = resp.ok_or_else(|| anyhow!("Empty guild channels response"))?;
        let channels: Vec<DiscordChannel> =
            serde_json::from_value(val).context("Failed to deserialize guild channels")?;

        for ch in &channels {
            let mut c = ch.clone();
            if c.guild_id.is_none() {
                c.guild_id = Some(guild_id.to_string());
            }
            let _ = self.db.upsert_channel(&c);
        }

        Ok(channels)
    }

    pub async fn list_dm_channels(&self) -> Result<Vec<DmChannel>> {
        let resp = self
            .rest(reqwest::Method::GET, "/users/@me/channels", None, None)
            .await?;

        let val = resp.ok_or_else(|| anyhow!("Empty dm channels response"))?;
        let raw_arr: Vec<serde_json::Value> =
            serde_json::from_value(val).context("Failed to deserialize DM channels array")?;

        let mut result = Vec::new();
        for item in raw_arr {
            let kind = item.get("type").and_then(|v| v.as_i64()).unwrap_or(1) as i32;
            let id = match item.get("id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            if kind == 1 {
                // Direct Message
                if let Some(recipients) = item.get("recipients").and_then(|v| v.as_array()) {
                    if let Some(rec_val) = recipients.first() {
                        if let Ok(rec_user) = serde_json::from_value::<DiscordUser>(rec_val.clone())
                        {
                            let dm = DmChannel {
                                id: id.clone(),
                                recipient: rec_user,
                                kind: 1,
                            };
                            let _ = self.db.upsert_dm_channel(&dm);
                            result.push(dm);
                        }
                    }
                }
            } else if kind == 3 {
                // Group DM
                let channel_name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());

                let mut first_rec_user = None;
                if let Some(recipients) = item.get("recipients").and_then(|v| v.as_array()) {
                    if let Some(rec_val) = recipients.first() {
                        first_rec_user =
                            serde_json::from_value::<DiscordUser>(rec_val.clone()).ok();
                    }
                }

                let final_rec = if let Some(name) = channel_name {
                    DiscordUser {
                        id: id.clone(),
                        username: name,
                        global_name: None,
                        discriminator: None,
                        avatar: None,
                    }
                } else {
                    first_rec_user.unwrap_or_else(|| DiscordUser {
                        id: id.clone(),
                        username: "Group DM".to_string(),
                        global_name: None,
                        discriminator: None,
                        avatar: None,
                    })
                };

                let dm = DmChannel {
                    id: id.clone(),
                    recipient: final_rec,
                    kind: 3,
                };
                let _ = self.db.upsert_dm_channel(&dm);
                result.push(dm);
            }
        }

        Ok(result)
    }

    pub async fn get_channel_messages(
        &self,
        channel_id: &str,
        limit: usize,
        before: Option<&str>,
        after: Option<&str>,
    ) -> Result<Vec<DiscordMessage>> {
        let capped_limit = limit.clamp(1, 100);
        let path = format!("/channels/{}/messages", channel_id);

        let mut query = vec![("limit".to_string(), capped_limit.to_string())];
        if let Some(b) = before {
            query.push(("before".to_string(), b.to_string()));
        }
        if let Some(a) = after {
            query.push(("after".to_string(), a.to_string()));
        }

        let resp = self
            .rest(reqwest::Method::GET, &path, Some(&query), None)
            .await?;

        let val = resp.ok_or_else(|| anyhow!("Empty messages response"))?;
        let messages: Vec<DiscordMessage> =
            serde_json::from_value(val).context("Failed to deserialize messages array")?;

        Ok(messages)
    }

    pub async fn fetch_messages(
        &self,
        channel_id: &str,
        limit: usize,
        before: Option<&str>,
    ) -> Result<Vec<DiscordMessage>> {
        let capped_limit = limit.clamp(1, 100);
        let path = format!("/channels/{}/messages", channel_id);

        let mut query = vec![("limit".to_string(), capped_limit.to_string())];
        if let Some(b) = before {
            query.push(("before".to_string(), b.to_string()));
        }

        let resp = self
            .rest(reqwest::Method::GET, &path, Some(&query), None)
            .await?;

        let val = resp.ok_or_else(|| anyhow!("Empty messages response"))?;
        let messages: Vec<DiscordMessage> =
            serde_json::from_value(val).context("Failed to deserialize messages array")?;

        for msg in &messages {
            let _ = self.db.upsert_message(msg);
        }

        Ok(messages)
    }

    pub async fn search_messages(
        &self,
        text: &str,
        guild_id: Option<&str>,
        channel_id: Option<&str>,
        author_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiscordMessage>> {
        let path = if let Some(channel) = channel_id {
            format!("/channels/{}/messages/search", channel)
        } else if let Some(guild) = guild_id {
            format!("/guilds/{}/messages/search", guild)
        } else {
            return Err(anyhow!("search requires a server or channel filter"));
        };

        let mut query = vec![
            ("content".to_string(), text.to_string()),
            ("limit".to_string(), limit.min(25).to_string()),
            ("offset".to_string(), "0".to_string()),
        ];
        if let Some(author) = author_id {
            query.push(("author_id".to_string(), author.to_string()));
        }

        const MAX_RETRIES: usize = 3;
        let mut retries = 0;

        let val = loop {
            let resp = self
                .rest(reqwest::Method::GET, &path, Some(&query), None)
                .await?;
            let value = resp.unwrap_or(serde_json::Value::Null);

            if value.get("code").and_then(|c| c.as_i64()) == Some(110000) {
                if retries >= MAX_RETRIES {
                    info!(
                        "Search index not available after {} retries for {}",
                        retries, path
                    );
                    return Err(anyhow!("Discord search index unavailable after retries"));
                }
                retries += 1;
                let retry_after = value
                    .get("retry_after")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0)
                    .clamp(0.0, 60.0);
                tokio::time::sleep(Duration::from_secs_f64(retry_after)).await;
                continue;
            }

            break value;
        };

        let messages = parse_search_response(&val)?;
        for msg in &messages {
            let _ = self.db.upsert_user(&msg.author);
            let _ = self.db.upsert_message(msg);
        }

        Ok(messages)
    }

    pub async fn sync_recent(
        &self,
        guild_limit: usize,
        channels_per_guild: usize,
        dm_limit: usize,
        per_channel: usize,
    ) -> Result<SyncReport> {
        let mut report = SyncReport::default();

        let guilds = match self.list_guilds().await {
            Ok(g) => g,
            Err(e) => {
                report.errors.push(format!("list_guilds failed: {}", e));
                Vec::new()
            }
        };

        let dms = match self.list_dm_channels().await {
            Ok(d) => d,
            Err(e) => {
                report
                    .errors
                    .push(format!("list_dm_channels failed: {}", e));
                Vec::new()
            }
        };

        for guild in guilds.into_iter().take(guild_limit) {
            match self.list_channels(&guild.id).await {
                Ok(channels) => {
                    let text_channels: Vec<_> = channels
                        .into_iter()
                        .filter(|c| c.kind == 0 || c.kind == 5)
                        .take(channels_per_guild)
                        .collect();

                    for channel in text_channels {
                        report.channels_fetched += 1;
                        match self.fetch_messages(&channel.id, per_channel, None).await {
                            Ok(msgs) => {
                                report.messages_fetched += msgs.len();
                            }
                            Err(e) => {
                                report.errors.push(format!(
                                    "fetch_messages for channel {} (guild {}): {}",
                                    channel.id, guild.id, e
                                ));
                            }
                        }
                    }
                }
                Err(e) => {
                    report
                        .errors
                        .push(format!("list_channels for guild {}: {}", guild.id, e));
                }
            }
        }

        for dm in dms.into_iter().take(dm_limit) {
            report.channels_fetched += 1;
            match self.fetch_messages(&dm.id, per_channel, None).await {
                Ok(msgs) => {
                    report.messages_fetched += msgs.len();
                }
                Err(e) => {
                    report
                        .errors
                        .push(format!("fetch_messages for DM {}: {}", dm.id, e));
                }
            }
        }

        Ok(report)
    }

    pub async fn validate_token(token: &str) -> Result<DiscordUser> {
        if token.trim().is_empty() {
            return Err(anyhow!("Discord token missing"));
        }

        let default_headers = build_rest_headers()?;

        let client = reqwest::Client::builder()
            .default_headers(default_headers)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .build()
            .context("Failed to construct reqwest client")?;

        let url = format!("{}/users/@me", DEFAULT_BASE_URL);
        let resp = client
            .get(&url)
            .header(AUTHORIZATION, token)
            .send()
            .await
            .map_err(|e| {
                anyhow!(
                    "Network error: {}",
                    e.to_string().replace(token, "[REDACTED]")
                )
            })?;

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            let sanitized = err_text.replace(token, "[REDACTED]");
            return Err(anyhow!(
                "Discord API returned status {}: {}",
                status.as_u16(),
                sanitized
            ));
        }

        let user: DiscordUser = resp.json().await.map_err(|e| {
            anyhow!(
                "Failed to parse user JSON: {}",
                e.to_string().replace(token, "[REDACTED]")
            )
        })?;

        Ok(user)
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Wake any gateway task parked on a socket read, heartbeat tick, or
        // reconnect backoff so it unwinds now instead of waiting up to a full
        // heartbeat interval (41s) or backoff (60s), or forever on a half-open
        // socket.
        self.shutdown_notify.notify_waiters();
    }

    /// Sleep for `duration` unless a shutdown is signaled first, in which case
    /// return `true` immediately. The waiter is force-registered before the
    /// flag is checked so a concurrent `stop()` cannot lose its wakeup.
    async fn sleep_or_shutdown(&self, duration: Duration) -> bool {
        let mut notified = Box::pin(self.shutdown_notify.notified());
        let _ = (&mut notified).now_or_never();
        if self.shutdown.load(Ordering::SeqCst) {
            return true;
        }
        tokio::select! {
            biased;
            _ = &mut notified => true,
            _ = tokio::time::sleep(duration) => false,
        }
    }

    pub fn start_gateway(&self, token: String) -> tokio::task::JoinHandle<()> {
        let client = self.clone();
        tokio::spawn(async move {
            client.run_gateway_loop(token).await;
        })
    }

    async fn run_gateway_loop(&self, token: String) {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(60);

        let mut session_id: Option<String> = None;
        let mut resume_gateway_url: Option<String> = None;
        let mut last_sequence: Option<i64> = None;

        // Register the shutdown waiter before checking the flag: stop() stores
        // the flag then calls notify_waiters(), so registering first makes it
        // impossible to lose the wakeup. Keep the future pinned for the whole
        // loop and prefer it in every select via `biased`.
        let mut shutdown = Box::pin(self.shutdown_notify.notified());
        let _ = (&mut shutdown).now_or_never();
        if self.shutdown.load(Ordering::SeqCst) {
            info!("[gateway] Gateway task finished shutdown");
            return;
        }

        'outer: while !self.shutdown.load(Ordering::SeqCst) {
            let ws_endpoint = resume_gateway_url.as_deref().unwrap_or(GATEWAY_URL);

            let connect_url = if ws_endpoint.contains("v=10") {
                ws_endpoint.to_string()
            } else if ws_endpoint.contains('?') {
                format!("{}&v=10&encoding=json", ws_endpoint)
            } else {
                format!("{}/?v=10&encoding=json", ws_endpoint.trim_end_matches('/'))
            };

            info!("[gateway] Connecting to gateway...");

            let connect_res = tokio::select! {
                biased;
                _ = &mut shutdown => {
                    info!("[gateway] Shutdown requested while connecting");
                    break 'outer;
                }
                res = tokio_tungstenite::connect_async(&connect_url) => res,
            };
            let (ws_stream, _) = match connect_res {
                Ok(res) => {
                    info!("[gateway] Connected to Discord Gateway");
                    backoff = Duration::from_secs(1); // Reset backoff on successful connect
                    res
                }
                Err(e) => {
                    let sanitized = self.sanitize(&e.to_string());
                    warn!(
                        "[gateway] Connection error: {}. Retrying in {:?}...",
                        sanitized, backoff
                    );
                    if self.sleep_or_shutdown(backoff).await {
                        break 'outer;
                    }
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }
            };

            let (mut write, mut read) = ws_stream.split();

            let mut heartbeat_interval_ms: Option<u64> = None;
            let mut heartbeat_timer = tokio::time::interval(Duration::from_secs(41));
            let mut heartbeat_ack_received = true;

            let mut should_reconnect = false;

            while !self.shutdown.load(Ordering::SeqCst) {
                tokio::select! {
                    biased;
                    _ = &mut shutdown => {
                        info!("[gateway] Shutdown requested; closing gateway connection");
                        break 'outer;
                    }

                    _ = heartbeat_timer.tick(), if heartbeat_interval_ms.is_some() => {
                        if !heartbeat_ack_received {
                            warn!("[gateway] Heartbeat ACK not received in time; reconnecting");
                            should_reconnect = true;
                            break;
                        }
                        heartbeat_ack_received = false;
                        let hb_msg = json!({
                            "op": 1,
                            "d": last_sequence
                        });
                        if let Err(e) = write.send(Message::Text(hb_msg.to_string().into())).await {
                            let s = self.sanitize(&e.to_string());
                            warn!("[gateway] Failed to send heartbeat: {}", s);
                            should_reconnect = true;
                            break;
                        }
                    }

                    msg = read.next() => {
                        let msg = match msg {
                            Some(Ok(m)) => m,
                            Some(Err(e)) => {
                                let s = self.sanitize(&e.to_string());
                                warn!("[gateway] WebSocket read error: {}", s);
                                should_reconnect = true;
                                break;
                            }
                            None => {
                                warn!("[gateway] WebSocket stream ended");
                                should_reconnect = true;
                                break;
                            }
                        };

                        match msg {
                            Message::Text(text) => {
                                let val: serde_json::Value = match serde_json::from_str(&text) {
                                    Ok(v) => v,
                                    Err(_) => continue,
                                };

                                let op = val.get("op").and_then(|v| v.as_i64()).unwrap_or(-1);
                                if let Some(s) = val.get("s").and_then(|v| v.as_i64()) {
                                    last_sequence = Some(s);
                                }

                                match op {
                                    // HELLO
                                    10 => {
                                        let interval = val.get("d")
                                            .and_then(|d| d.get("heartbeat_interval"))
                                            .and_then(|i| i.as_u64())
                                            .unwrap_or(41250);

                                        heartbeat_interval_ms = Some(interval);
                                        heartbeat_ack_received = true;
                                        heartbeat_timer = tokio::time::interval(Duration::from_millis(interval));
                                        heartbeat_timer.tick().await; // first tick is immediate

                                        // Identify or Resume
                                        if let (Some(ref sid), Some(seq)) = (&session_id, last_sequence) {
                                            info!("[gateway] Sending RESUME session {}", sid);
                                            let resume_payload = json!({
                                                "op": 6,
                                                "d": {
                                                    "token": token,
                                                    "session_id": sid,
                                                    "seq": seq
                                                }
                                            });
                                            if let Err(e) = write.send(Message::Text(resume_payload.to_string().into())).await {
                                                let s = self.sanitize(&e.to_string());
                                                warn!("[gateway] Failed to send resume: {}", s);
                                                should_reconnect = true;
                                                break;
                                            }
                                        } else {
                                            // Enforce rate limit of 1 identify per 5s
                                            {
                                                let mut last_id = self.last_identify.lock().await;
                                                let elapsed = last_id.elapsed();
                                                if elapsed < Duration::from_secs(5)
                                                    && self.sleep_or_shutdown(Duration::from_secs(5) - elapsed).await
                                                {
                                                    should_reconnect = false;
                                                    break;
                                                }
                                                *last_id = Instant::now();
                                            }

                                            info!("[gateway] Sending IDENTIFY");
                                            let identify_payload = json!({
                                                "op": 2,
                                                "d": {
                                                    "token": token,
                                                    "capabilities": 16381,
                                                    "properties": {
                                                        "os": "Windows",
                                                        "browser": "Chrome",
                                                        "device": "",
                                                        "system_locale": "en-US",
                                                        "browser_user_agent": BROWSER_USER_AGENT,
                                                        "browser_version": "133.0.0.0",
                                                        "os_version": "10",
                                                        "referrer": "",
                                                        "referring_domain": "",
                                                        "referrer_current": "",
                                                        "referring_domain_current": "",
                                                        "release_channel": "stable",
                                                        "client_build_number": 367200,
                                                        "client_event_source": null
                                                    },
                                                    "presence": {
                                                        "status": "unknown",
                                                        "since": 0,
                                                        "activities": [],
                                                        "afk": false
                                                    },
                                                    "compress": false
                                                }
                                            });
                                            if let Err(e) = write.send(Message::Text(identify_payload.to_string().into())).await {
                                                let s = self.sanitize(&e.to_string());
                                                warn!("[gateway] Failed to send identify: {}", s);
                                                should_reconnect = true;
                                                break;
                                            }
                                        }
                                    }

                                    // HEARTBEAT requested
                                    1 => {
                                        let hb_msg = json!({
                                            "op": 1,
                                            "d": last_sequence
                                        });
                                        let _ = write.send(Message::Text(hb_msg.to_string().into())).await;
                                    }

                                    // HEARTBEAT ACK
                                    11 => {
                                        heartbeat_ack_received = true;
                                    }

                                    // RECONNECT
                                    7 => {
                                        info!("[gateway] Received OP 7 Reconnect");
                                        should_reconnect = true;
                                        break;
                                    }

                                    // INVALID SESSION
                                    9 => {
                                        let resumable = val.get("d").and_then(|d| d.as_bool()).unwrap_or(false);
                                        warn!("[gateway] Received OP 9 Invalid Session (resumable: {})", resumable);
                                        if !resumable {
                                            session_id = None;
                                            last_sequence = None;
                                            resume_gateway_url = None;
                                        }
                                        if self.sleep_or_shutdown(Duration::from_secs(4)).await {
                                            should_reconnect = false;
                                            break;
                                        }
                                        should_reconnect = true;
                                        break;
                                    }

                                    // DISPATCH
                                    0 => {
                                        let event_type = val.get("t").and_then(|t| t.as_str()).unwrap_or("");
                                        let data = val.get("d").cloned().unwrap_or(serde_json::Value::Null);

                                        self.handle_dispatch(
                                            event_type,
                                            data,
                                            &mut session_id,
                                            &mut resume_gateway_url,
                                        ).await;
                                    }

                                    _ => {}
                                }
                            }
                            Message::Ping(payload) => {
                                let _ = write.send(Message::Pong(payload)).await;
                            }
                            Message::Pong(_) => {}
                            Message::Close(frame) => {
                                warn!("[gateway] Received Close frame: {:?}", frame);
                                should_reconnect = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }

            if should_reconnect
                && !self.shutdown.load(Ordering::SeqCst)
                && self.sleep_or_shutdown(Duration::from_secs(1)).await
            {
                break;
            }
        }

        info!("[gateway] Gateway task finished shutdown");
    }

    async fn handle_dispatch(
        &self,
        event_type: &str,
        data: serde_json::Value,
        session_id: &mut Option<String>,
        resume_gateway_url: &mut Option<String>,
    ) {
        match event_type {
            "READY" => {
                info!("[gateway] Received READY event");
                if let Some(sid) = data.get("session_id").and_then(|s| s.as_str()) {
                    *session_id = Some(sid.to_string());
                }
                if let Some(rurl) = data.get("resume_gateway_url").and_then(|u| u.as_str()) {
                    *resume_gateway_url = Some(rurl.to_string());
                }

                // Cache user
                if let Some(user_val) = data.get("user") {
                    if let Ok(user) = serde_json::from_value::<DiscordUser>(user_val.clone()) {
                        let _ = self.db.upsert_user(&user);
                        let mut me = self.me.lock().await;
                        *me = Some(user);
                    }
                }

                // Guilds with names only (skip nameless stubs)
                if let Some(guilds) = data.get("guilds").and_then(|g| g.as_array()) {
                    for g_val in guilds {
                        let name = g_val.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        if !name.trim().is_empty() {
                            if let Ok(guild) = serde_json::from_value::<DiscordGuild>(g_val.clone())
                            {
                                let _ = self.db.upsert_guild(&guild);
                            }
                        }
                    }
                }

                // Private channels with recipients
                if let Some(private_channels) =
                    data.get("private_channels").and_then(|c| c.as_array())
                {
                    for ch_val in private_channels {
                        let kind = ch_val.get("type").and_then(|t| t.as_i64()).unwrap_or(1) as i32;
                        let id = match ch_val.get("id").and_then(|i| i.as_str()) {
                            Some(id) => id.to_string(),
                            None => continue,
                        };

                        if let Some(recipients) =
                            ch_val.get("recipients").and_then(|r| r.as_array())
                        {
                            if kind == 1 {
                                if let Some(first_rec) = recipients.first() {
                                    if let Ok(u) =
                                        serde_json::from_value::<DiscordUser>(first_rec.clone())
                                    {
                                        let dm = DmChannel {
                                            id,
                                            recipient: u,
                                            kind: 1,
                                        };
                                        let _ = self.db.upsert_dm_channel(&dm);
                                    }
                                }
                            } else if kind == 3 {
                                let channel_name = ch_val
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty());

                                let mut first_rec_user = None;
                                if let Some(first_rec) = recipients.first() {
                                    first_rec_user =
                                        serde_json::from_value::<DiscordUser>(first_rec.clone())
                                            .ok();
                                }

                                let final_rec = if let Some(name) = channel_name {
                                    DiscordUser {
                                        id: id.clone(),
                                        username: name,
                                        global_name: None,
                                        discriminator: None,
                                        avatar: None,
                                    }
                                } else {
                                    first_rec_user.unwrap_or_else(|| DiscordUser {
                                        id: id.clone(),
                                        username: "Group DM".to_string(),
                                        global_name: None,
                                        discriminator: None,
                                        avatar: None,
                                    })
                                };

                                let dm = DmChannel {
                                    id,
                                    recipient: final_rec,
                                    kind: 3,
                                };
                                let _ = self.db.upsert_dm_channel(&dm);
                            }
                        }
                    }
                }
            }

            "RESUMED" => {
                info!("[gateway] Successfully RESUMED session");
            }

            "GUILD_CREATE" | "GUILD_UPDATE" => {
                let name = data.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if !name.trim().is_empty() {
                    if let Ok(guild) = serde_json::from_value::<DiscordGuild>(data.clone()) {
                        let _ = self.db.upsert_guild(&guild);
                    }
                }

                let guild_id = data
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                if !guild_id.is_empty() {
                    if let Some(channels) = data.get("channels").and_then(|c| c.as_array()) {
                        for ch_val in channels {
                            if let Ok(mut ch) =
                                serde_json::from_value::<DiscordChannel>(ch_val.clone())
                            {
                                if ch.guild_id.is_none() {
                                    ch.guild_id = Some(guild_id.clone());
                                }
                                let _ = self.db.upsert_channel(&ch);
                            }
                        }
                    }
                }
            }

            "CHANNEL_CREATE" | "CHANNEL_UPDATE" => {
                let kind = data.get("type").and_then(|t| t.as_i64()).unwrap_or(0) as i32;
                if kind == 1 {
                    let id = data
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(recipients) = data.get("recipients").and_then(|r| r.as_array()) {
                        if let Some(rec_val) = recipients.first() {
                            if let Ok(u) = serde_json::from_value::<DiscordUser>(rec_val.clone()) {
                                let dm = DmChannel {
                                    id,
                                    recipient: u,
                                    kind: 1,
                                };
                                let _ = self.db.upsert_dm_channel(&dm);
                            }
                        }
                    }
                } else if let Ok(ch) = serde_json::from_value::<DiscordChannel>(data) {
                    let _ = self.db.upsert_channel(&ch);
                }
            }

            "MESSAGE_CREATE" => {
                if let Ok(msg) = serde_json::from_value::<DiscordMessage>(data) {
                    let _ = self.db.upsert_message(&msg);
                }
            }

            "MESSAGE_UPDATE" => {
                if let Ok(partial) = serde_json::from_value::<PartialDiscordMessage>(data.clone()) {
                    let _ = self.db.update_partial_message(&partial);
                } else if let Ok(msg) = serde_json::from_value::<DiscordMessage>(data) {
                    let _ = self.db.upsert_message(&msg);
                }
            }

            "MESSAGE_DELETE" => {
                if let Some(msg_id) = data.get("id").and_then(|i| i.as_str()) {
                    let _ = self.db.delete_message(msg_id);
                }
            }

            _ => {
                // Ignore other events like PRESENCE_UPDATE, TYPING_START, etc.
            }
        }
    }
}
