//! Bot API file downloads.
//!
//! `getFile` names a server-side path, and the bytes are served at
//! `/file/bot<token>/<file_path>`: the URL embeds the bot token. Every
//! transport error is stripped of its URL before it is classified, and
//! [`FileError`] carries no strings, so neither errors nor tracing can carry
//! the token. This module has its own error type so the transport does not
//! depend on the media layer that uses it.
use super::{valid_token, TelegramClient};
use cmaeg_core::channel::Secret;
use serde_json::{json, Value};
use tokio::time::Instant;

/// The longest file path Telegram is expected to name.
const MAX_FILE_PATH_BYTES: usize = 256;

/// Why a Telegram file could not be obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileError {
    /// Telegram does not serve files this large to bots.
    TooLarge,
    NotFound,
    RateLimited,
    /// Refused locally before any request, such as an unsafe file path.
    Rejected,
    /// Any other failure, including transport errors.
    Unavailable,
}

/// `getFile`'s answer. Telegram omits the path for a file it will not serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramFile {
    pub file_path: Option<String>,
    pub file_size: Option<u64>,
}

impl TelegramClient {
    /// Resolve a `file_id` to the path its bytes are served at.
    pub async fn get_file(&self, token: &Secret, file_id: &str) -> Result<TelegramFile, FileError> {
        let (status, value) = self
            .call_raw(token, "getFile", &json!({ "file_id": file_id }))
            .await
            .map_err(|_| FileError::Unavailable)?;
        match status.as_u16() {
            200 if value["ok"] == true => Ok(TelegramFile {
                file_path: value["result"]["file_path"].as_str().map(str::to_owned),
                file_size: value["result"]["file_size"].as_u64(),
            }),
            400 if too_big(&value) => Err(FileError::TooLarge),
            400 | 404 => Err(FileError::NotFound),
            429 => Err(FileError::RateLimited),
            _ => Err(FileError::Unavailable),
        }
    }

    /// Start downloading `file_path`; the response body is the file. The
    /// request, body included, ends at `deadline`.
    pub async fn download_file(
        &self,
        token: &Secret,
        file_path: &str,
        deadline: Instant,
    ) -> Result<reqwest::Response, FileError> {
        if !valid_file_path(file_path) || !valid_token(token.as_str()) {
            return Err(FileError::Rejected);
        }
        let url = self
            .origin
            .join(&format!("/file/bot{}/{file_path}", token.as_str()))
            .map_err(|_| FileError::Rejected)?;
        let response = self
            .http
            .get(url)
            .timeout(deadline.saturating_duration_since(Instant::now()))
            .send()
            .await
            .map_err(|error| download_failure(error.without_url()))?;
        match response.status().as_u16() {
            200 => Ok(response),
            404 => Err(FileError::NotFound),
            429 => Err(FileError::RateLimited),
            _ => Err(FileError::Unavailable),
        }
    }
}

/// Telegram reports an oversized file as a 400 whose description says so.
fn too_big(value: &Value) -> bool {
    value["description"]
        .as_str()
        .is_some_and(|description| description.to_ascii_lowercase().contains("file is too big"))
}

/// `^[A-Za-z0-9_./-]{1,256}$` with no `..` segment and no leading `/`, so the
/// path cannot climb out of the token's file directory; percent signs, and so
/// encoded dots, are outside the alphabet.
fn valid_file_path(path: &str) -> bool {
    (1..=MAX_FILE_PATH_BYTES).contains(&path.len())
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'-'))
        && !path.starts_with('/')
        && !path.split('/').any(|segment| segment == "..")
}

/// Classifies a download transport failure. It only receives errors already
/// stripped of their token-bearing URL.
fn download_failure(_error: reqwest::Error) -> FileError {
    FileError::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_paths_stay_inside_the_token_directory() {
        for path in [
            "photos/file_1.jpg",
            "documents/report-v2.pdf",
            "voice/a.b.c.oga",
            "x",
        ] {
            assert!(valid_file_path(path), "{path}");
        }
        for path in [
            "",
            "/etc/passwd",
            "../x",
            "photos/../../x",
            "photos/..",
            "%2e%2e/x",
            "photos/a b.jpg",
            "photos/a?b",
            "photos\\a",
            &"a".repeat(257),
        ] {
            assert!(!valid_file_path(path), "{path}");
        }
    }

    #[test]
    fn only_a_too_big_description_is_too_large() {
        assert!(too_big(
            &json!({"description": "Bad Request: file is too big"})
        ));
        assert!(too_big(
            &json!({"description": "Bad Request: FILE IS TOO BIG"})
        ));
        assert!(!too_big(
            &json!({"description": "Bad Request: wrong file_id"})
        ));
        assert!(!too_big(&json!({})));
    }
}
