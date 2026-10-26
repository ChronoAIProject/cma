//! Native Discord REST. Bot credentials go only to the fixed official API origin.
use crate::{
    channels::resolver::ResolvedChannel,
    media::{
        body::{read_capped, Overflow},
        FetchError,
    },
};
use cmaeg_core::channel::{valid_discord_id as valid_id, Secret, Source};
use reqwest::{Client, Url};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestError {
    Rejected,
    NotFound,
    RateLimited(u64),
    Unavailable,
    Unknown,
}
type Cooldowns = Arc<Mutex<HashMap<String, Instant>>>;
fn shared_cooldowns() -> Cooldowns {
    static COOLDOWNS: OnceLock<Cooldowns> = OnceLock::new();
    COOLDOWNS
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}
#[derive(Clone)]
pub struct DiscordRest {
    client: Client,
    origin: Url,
    cooldowns: Cooldowns,
    identities: Arc<Mutex<HashMap<String, (String, Instant)>>>,
}
/// Bytes are prepared before delivery commits; credentials never enter file parts.
pub struct PreparedFile {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub mime: String,
    pub caption: Option<String>,
}
impl DiscordRest {
    pub fn new() -> Result<Self, crate::ConfigError> {
        let error = || crate::ConfigError::InvalidManagement {
            detail: "native Discord HTTP client unavailable".into(),
        };
        Ok(Self {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| error())?,
            origin: Url::parse("https://discord.com/api/v10/").map_err(|_| error())?,
            cooldowns: shared_cooldowns(),
            identities: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    /// Bind every token revision to its actual bot before a native write or read.
    /// Only a hash and the public bot ID are cached, bounded to five minutes.
    pub(crate) async fn verify_bot(&self, token: &Secret, expected: &str) -> Result<(), RestError> {
        if !valid_id(expected) {
            return Err(RestError::Rejected);
        }
        let key = crate::channels::definition::hash_secret(token.as_str());
        {
            let mut identities = self.identities.lock().unwrap();
            identities.retain(|_, (_, until)| *until > Instant::now());
            if let Some((bot, _)) = identities.get(&key) {
                return if bot == expected {
                    Ok(())
                } else {
                    Err(RestError::Rejected)
                };
            }
            if identities.len() >= 512 {
                return Err(RestError::Unavailable);
            }
        }
        self.ready(token)?;
        let response = self
            .client
            .get(self.url("users/@me")?)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| RestError::Unavailable)?;
        let body = self.decode(response, token, false).await?;
        let bot = body["id"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or(RestError::Rejected)?;
        if body["bot"].as_bool() != Some(true) {
            return Err(RestError::Rejected);
        }
        let mut identities = self.identities.lock().unwrap();
        if identities.len() < 512 {
            identities.insert(key, (bot.into(), Instant::now() + Duration::from_secs(300)));
        }
        if bot == expected {
            Ok(())
        } else {
            Err(RestError::Rejected)
        }
    }
    pub async fn message(
        &self,
        token: &Secret,
        channel: &str,
        message: &str,
    ) -> Result<Value, RestError> {
        if !valid_id(channel) || !valid_id(message) {
            return Err(RestError::Rejected);
        }
        self.ready(token)?;
        let response = self
            .client
            .get(self.url(&format!("channels/{channel}/messages/{message}"))?)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| RestError::Unavailable)?;
        let body = self.decode(response, token, false).await?;
        if body["id"].as_str() != Some(message) || body["channel_id"].as_str() != Some(channel) {
            return Err(RestError::Rejected);
        }
        Ok(body)
    }
    /// Never retry ambiguous writes: an error after dispatch can mean Discord accepted it.
    pub async fn send(
        &self,
        token: &Secret,
        bot: &str,
        channel: &str,
        reply_to: Option<&str>,
        text: &str,
        files: &[PreparedFile],
    ) -> Result<String, RestError> {
        if !valid_id(bot)
            || !valid_id(channel)
            || reply_to.is_some_and(|id| !valid_id(id))
            || text.chars().map(char::len_utf16).sum::<usize>() > 2000
            || (text.is_empty() && files.is_empty())
            || files.len() > 4
        {
            return Err(RestError::Rejected);
        }
        self.ready(token)?;
        self.verify_bot(token, bot).await?;
        self.ready(token)?;
        let mut payload =
            json!({"content":text,"allowed_mentions":{"parse":[],"replied_user":false}});
        if let Some(message) = reply_to {
            payload["message_reference"] =
                json!({"message_id":message,"channel_id":channel,"fail_if_not_exists":false});
        }
        let mut request = self
            .client
            .post(self.url(&format!("channels/{channel}/messages"))?)
            .header("Authorization", format!("Bot {}", token.as_str()));
        if files.is_empty() {
            request = request.json(&payload);
        } else {
            payload["attachments"] = Value::Array(
                files
                    .iter()
                    .enumerate()
                    .map(|(i, file)| {
                        let mut attachment = json!({"id":i,"filename":file.filename});
                        if let Some(caption) = &file.caption {
                            attachment["description"] = json!(caption);
                        }
                        attachment
                    })
                    .collect(),
            );
            let mut form =
                reqwest::multipart::Form::new().text("payload_json", payload.to_string());
            for (i, file) in files.iter().enumerate() {
                let part = reqwest::multipart::Part::bytes(file.bytes.clone())
                    .file_name(file.filename.clone())
                    .mime_str(&file.mime)
                    .map_err(|_| RestError::Rejected)?;
                form = form.part(format!("files[{i}]"), part);
            }
            request = request.multipart(form);
        }
        let response = request.send().await.map_err(|_| RestError::Unknown)?;
        let body = self.decode(response, token, true).await?;
        if body["channel_id"].as_str() != Some(channel)
            || body["author"]["id"].as_str() != Some(bot)
        {
            return Err(RestError::Unknown);
        }
        body["id"]
            .as_str()
            .filter(|id| valid_id(id))
            .map(str::to_owned)
            .ok_or(RestError::Unknown)
    }
    fn url(&self, path: &str) -> Result<Url, RestError> {
        self.origin.join(path).map_err(|_| RestError::Rejected)
    }
    fn ready(&self, token: &Secret) -> Result<(), RestError> {
        let mut cooldowns = self.cooldowns.lock().unwrap();
        cooldowns.retain(|_, until| *until > Instant::now());
        if let Some(until) =
            cooldowns.get(&crate::channels::definition::hash_secret(token.as_str()))
        {
            return Err(RestError::RateLimited(
                until
                    .saturating_duration_since(Instant::now())
                    .as_secs()
                    .saturating_add(1),
            ));
        }
        if cooldowns.len() >= 1024 {
            return Err(RestError::Unavailable);
        }
        Ok(())
    }
    fn cooldown(&self, token: &Secret, seconds: f64) {
        let seconds = if seconds.is_finite() && seconds > 0.0 {
            seconds.ceil().clamp(1.0, 3600.0) as u64
        } else {
            60
        };
        let key = crate::channels::definition::hash_secret(token.as_str());
        let mut cooldowns = self.cooldowns.lock().unwrap();
        cooldowns.retain(|_, until| *until > Instant::now());
        if cooldowns.len() < 1024 || cooldowns.contains_key(&key) {
            cooldowns.insert(key, Instant::now() + Duration::from_secs(seconds));
        }
    }
    async fn decode(
        &self,
        response: reqwest::Response,
        token: &Secret,
        write: bool,
    ) -> Result<Value, RestError> {
        let status = response.status().as_u16();
        let ambiguous = if write {
            RestError::Unknown
        } else {
            RestError::Unavailable
        };
        let header_delay = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok());
        let exhausted = response
            .headers()
            .get("x-ratelimit-remaining")
            .is_some_and(|v| v.as_bytes() == b"0");
        let reset_after = response
            .headers()
            .get("x-ratelimit-reset-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok());
        if exhausted {
            self.cooldown(token, reset_after.unwrap_or(1.0));
        }
        if status == 429 {
            self.cooldown(token, header_delay.unwrap_or(60.0));
        }
        if status == 404 {
            return Err(RestError::NotFound);
        }
        if status != 429 && !(200..300).contains(&status) {
            return Err(if status >= 500 || status == 408 {
                ambiguous
            } else {
                RestError::Rejected
            });
        }
        let bytes = read_capped(
            response,
            1024 * 1024,
            Overflow::Reject,
            Instant::now() + Duration::from_secs(30),
        )
        .await
        .map_err(|_| ambiguous)?;
        let body: Value = serde_json::from_slice(&bytes.bytes).map_err(|_| ambiguous)?;
        if status == 429 {
            let seconds = body["retry_after"]
                .as_f64()
                .or(header_delay)
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| v.ceil().clamp(1.0, 3600.0) as u64)
                .unwrap_or(60);
            self.cooldown(token, seconds as f64);
            return Err(RestError::RateLimited(seconds));
        }
        Ok(body)
    }
    #[cfg(test)]
    pub(crate) fn fixture(origin: &str) -> Self {
        let mut result = Self::new().unwrap();
        result.origin = Url::parse(origin).unwrap();
        result.cooldowns = Arc::new(Mutex::new(HashMap::new()));
        result
    }
}
pub(crate) fn token<'a>(channel: &'a ResolvedChannel, bot: &str) -> Option<&'a Secret> {
    channel
        .definition
        .sources
        .iter()
        .any(|source| matches!(source, Source::Discord { bot_id, .. } if bot_id == bot))
        .then_some(channel.definition.secrets.discord_bot_token.as_ref())
        .flatten()
}
pub(crate) fn fetch_error(error: RestError) -> FetchError {
    match error {
        RestError::Rejected => FetchError::Rejected,
        RestError::NotFound => FetchError::NotFound,
        RestError::RateLimited(_) => FetchError::RateLimited,
        _ => FetchError::Upstream,
    }
}
