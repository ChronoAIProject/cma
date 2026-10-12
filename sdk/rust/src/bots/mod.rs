//! Durable owner Bot management through CMA's CMAEG integration.
mod types;
use crate::{Client, IdempotencyKey, Result};
use reqwest::{Method, StatusCode};
pub use types::*;

/// Owner Bot operations; pending changes are observed by polling `get`.
pub struct Bots<'a>(&'a Client);
impl Client {
    /// Manage the caller's bots and discover their registered channel bots.
    pub fn bots(&self) -> Bots<'_> {
        Bots(self)
    }
}
impl Bots<'_> {
    /// List owned Event Gateway connections that can supply a consumer.
    pub async fn connections(&self) -> Result<Vec<ConsumerConnection>> {
        self.0.get(&["bots", "connections"], &[]).await
    }
    /// Admit creation with a durable owner-scoped idempotency key.
    pub async fn create(&self, body: &CreateBot, key: &IdempotencyKey) -> Result<Bot> {
        self.0
            .command(
                Method::POST,
                &["bots"],
                Some(body),
                key,
                StatusCode::CREATED,
            )
            .await
    }
    /// List owned bots in stable identity order.
    pub async fn list(&self, options: &BotListOptions) -> Result<BotPage> {
        let mut query = Vec::new();
        if let Some(after) = &options.after {
            query.push(("after", after.clone()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        self.0.get(&["bots"], &query).await
    }
    /// Read an owned bot, including durable operation progress.
    pub async fn get(&self, id: &str) -> Result<Bot> {
        self.0.get(&["bots", id], &[]).await
    }
    /// List active, personally owned Telegram bots registered with NyxID.
    pub async fn available(&self) -> Result<Vec<ExternalBot>> {
        self.0.get(&["bots", "nyxid-bots"], &[]).await
    }
    /// Run a synthetic provider turn; this does not send a channel message.
    pub async fn test(&self, id: &str) -> Result<BotTest> {
        self.0
            .operation(
                Method::POST,
                &["bots", id, "test"],
                None::<&()>,
                StatusCode::OK,
            )
            .await
    }
    /// Disable all source admission while retaining the configured policy.
    pub async fn disable(&self, id: &str) -> Result<Bot> {
        self.mutate(id, "disable").await
    }
    /// Restore the originally selected admission policy.
    pub async fn enable(&self, id: &str) -> Result<Bot> {
        self.mutate(id, "enable").await
    }
    /// Rotate both credentials through the durable reconciliation workflow.
    pub async fn rotate_keys(&self, id: &str) -> Result<Bot> {
        self.mutate(id, "rotate-keys").await
    }
    /// Admit deletion; poll until the resource returns 404 after completed cleanup.
    pub async fn delete(&self, id: &str) -> Result<Bot> {
        self.0
            .operation(
                Method::DELETE,
                &["bots", id],
                None::<&()>,
                StatusCode::ACCEPTED,
            )
            .await
    }
    async fn mutate(&self, id: &str, operation: &str) -> Result<Bot> {
        self.0
            .operation(
                Method::POST,
                &["bots", id, operation],
                None::<&()>,
                StatusCode::ACCEPTED,
            )
            .await
    }
}
