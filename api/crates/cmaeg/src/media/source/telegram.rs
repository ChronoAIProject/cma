//! Native Telegram files, downloaded with the channel's own bot token.
//!
//! The token goes only to Telegram. A file is fetched only while the current
//! channel definition still configures the bot that received it and still
//! holds its token, so removing either stops retrieval at once.
use super::{FetchRequest, MediaSource, SourceStream};
use crate::{
    media::{
        body::{read_capped, stream_capped, FetchedBody},
        FetchError,
    },
    telegram::{FileError, TelegramClient},
};
use cmaeg_core::{
    activity::{AttachmentSource, SourceKind},
    channel::{Secret, Source},
};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Duration};
use tokio::time::{timeout_at, Instant};

/// Telegram's `getFile` limit: larger files are never served to bots.
pub const TELEGRAM_MAX_BYTES: u64 = 20_000_000;
const TELEGRAM_DEADLINE: Duration = Duration::from_secs(30);

/// Fetches `telegram{file_id, file_unique_id}` locators.
pub struct TelegramFileSource {
    client: Arc<TelegramClient>,
}

impl TelegramFileSource {
    pub fn new(client: Arc<TelegramClient>) -> Self {
        Self { client }
    }

    /// The download response and the deadline its body must meet.
    async fn response(
        &self,
        request: &FetchRequest<'_>,
    ) -> Result<(reqwest::Response, Instant), FetchError> {
        let token = current_token(request).ok_or(FetchError::NotRetrievable)?;
        let Some(AttachmentSource::Telegram { file_id, .. }) = request.locator else {
            return Err(FetchError::NotRetrievable);
        };
        if request
            .declared_size
            .is_some_and(|size| size > TELEGRAM_MAX_BYTES)
        {
            return Err(FetchError::TooLarge);
        }
        let deadline = request.deadline.min(Instant::now() + TELEGRAM_DEADLINE);
        let file = timeout_at(deadline, self.client.get_file(token, file_id))
            .await
            .map_err(|_| FetchError::TimedOut)?
            .map_err(|error| classify(error, deadline))?;
        // Telegram omits the path for a file it will not serve to bots.
        let file_path = file.file_path.ok_or(FetchError::TooLarge)?;
        let response = timeout_at(
            deadline,
            self.client.download_file(token, &file_path, deadline),
        )
        .await
        .map_err(|_| FetchError::TimedOut)?
        .map_err(|error| classify(error, deadline))?;
        Ok((response, deadline))
    }
}

/// The bot token, only while the current definition still configures the
/// bot that received the activity.
fn current_token<'a>(request: &FetchRequest<'a>) -> Option<&'a Secret> {
    let SourceKind::Telegram { bot_id, .. } = &request.activity.source else {
        return None;
    };
    let definition = &request.channel.definition;
    let configured = definition.sources.iter().any(
        |source| matches!(source, Source::Telegram { bot_id: current, .. } if current == bot_id),
    );
    configured
        .then_some(definition.secrets.telegram_bot_token.as_ref())
        .flatten()
}

/// A transport failure once the deadline has passed is the deadline's doing.
fn classify(error: FileError, deadline: Instant) -> FetchError {
    match error {
        FileError::TooLarge => FetchError::TooLarge,
        FileError::NotFound => FetchError::NotFound,
        FileError::RateLimited => FetchError::RateLimited,
        FileError::Rejected => FetchError::Rejected,
        FileError::Unavailable if Instant::now() >= deadline => FetchError::TimedOut,
        FileError::Unavailable => FetchError::Upstream,
    }
}

impl MediaSource for TelegramFileSource {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        Box::pin(async move {
            let (response, deadline) = self.response(&request).await?;
            read_capped(
                response,
                request.cap.min(TELEGRAM_MAX_BYTES),
                request.overflow,
                deadline,
            )
            .await
        })
    }

    fn open<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async move {
            let (response, _) = self.response(&request).await?;
            stream_capped(response, request.cap.min(TELEGRAM_MAX_BYTES))
        })
    }
}
