//! Exact confirmed single-upload edits, separate from historical effects-only authority.
pub(crate) mod authority;
mod capabilities;
pub use capabilities::Capabilities;
mod execute;
pub(crate) mod model;
mod provider;
pub mod store;
mod verify;
use crate::{discord::DiscordRest, telegram::TelegramClient};
use cmaeg_core::problem::{Problem, ProblemCode};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub struct Client {
    pub(crate) telegram: Arc<TelegramClient>,
    pub(crate) discord: DiscordRest,
    pub telegram_issuance: bool,
    pub discord_issuance: bool,
    capacity: Semaphore,
}
impl Client {
    pub fn new(
        telegram: Arc<TelegramClient>,
        discord: DiscordRest,
        telegram_issuance: bool,
        discord_issuance: bool,
    ) -> Self {
        Self {
            telegram,
            discord,
            telegram_issuance,
            discord_issuance,
            capacity: Semaphore::new(16),
        }
    }
}
pub(crate) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
pub(crate) fn unavailable() -> Problem {
    Problem::new(ProblemCode::Internal, 503, "native_media_state_unavailable")
}
pub(crate) fn denied() -> Problem {
    Problem::new(
        ProblemCode::Forbidden,
        403,
        "native_media_authority_revoked",
    )
}
