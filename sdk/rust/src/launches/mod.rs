//! Explicit visitor admission, durable progress, and service connection handoffs.

use crate::{
    Client, EventOptions, IdempotencyKey, PageOptions, ResourcePage, Result, http::Request,
    resources::identity, triggers::CursorPage,
};
use reqwest::{Method, StatusCode};
mod connections;
mod events;
mod stream;
mod types;
pub use connections::*;
pub use events::*;
pub use stream::*;
pub use types::*;

/// Optional owner-only Launch filters and opaque pagination.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchListOptions {
    /// Filter by source Trigger owner identity.
    pub trigger: Option<String>,
    /// Filter by durable lifecycle state.
    pub state: Option<LaunchState>,
    /// Opaque cursor and page bound.
    pub page: PageOptions,
}

/// Launch and owner-only connection operations.
pub struct Launches<'a> {
    client: &'a Client,
}

impl Launches<'_> {
    /// Explicitly create or resume a Launch; never automatically retry this mutation.
    pub async fn create_or_resume(
        &self,
        public_id: &str,
        body: &CreateLaunchRequest,
        key: &IdempotencyKey,
    ) -> Result<Launch> {
        let segments = ["triggers", "public", public_id, "launches"];
        let mut request = Request::command(
            Method::POST,
            &segments,
            Some(body),
            key,
            StatusCode::CREATED,
        )?;
        request.statuses.push(StatusCode::ACCEPTED);
        self.client.request(request).await
    }
    /// Read one owner-only Launch, checking its identity.
    pub async fn read(&self, id: &str) -> Result<Launch> {
        let result: Launch = self.client.get(&["launches", id], &[]).await?;
        identity(&result.id, id)?;
        Ok(result)
    }
    /// List owned Launches with optional Trigger and lifecycle filters.
    pub async fn list(&self, options: &LaunchListOptions) -> Result<ResourcePage<Launch>> {
        let mut query = options.page.query();
        if let Some(trigger) = &options.trigger {
            query.push(("trigger", trigger.clone()));
        }
        if let Some(state) = options.state {
            query.push(("state", state.as_str().into()));
        }
        Ok(self
            .client
            .get::<CursorPage<Launch>>(&["launches"], &query)
            .await?
            .into())
    }
    /// Start after explicit review consent against the current trust disclosure.
    pub async fn start(
        &self,
        id: &str,
        body: &StartLaunchRequest,
        key: &IdempotencyKey,
    ) -> Result<Launch> {
        let result: Launch = self
            .client
            .command(
                Method::POST,
                &["launches", id, "start"],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await?;
        identity(&result.id, id)?;
        Ok(result)
    }
    /// Cancel a Launch only from a server-supported state.
    pub async fn cancel(&self, id: &str, key: &IdempotencyKey) -> Result<Launch> {
        let result: Launch = self
            .client
            .command(
                Method::POST,
                &["launches", id, "cancel"],
                None::<&()>,
                key,
                StatusCode::OK,
            )
            .await?;
        identity(&result.id, id)?;
        Ok(result)
    }
    /// Read fresh owner-only connection names and setup metadata.
    pub async fn requirements(&self, id: &str) -> Result<ConnectionRequirements> {
        self.client
            .get(&["launches", id, "requirements"], &[])
            .await
    }
    /// Read a bounded durable event page, validating identity, order, and cursor.
    pub async fn events(&self, id: &str, options: EventOptions) -> Result<LaunchEventPage> {
        let result: LaunchEventPage = self
            .client
            .get(&["launches", id, "events"], &options.query())
            .await?;
        identity(&result.launch_id, id)?;
        let mut cursor = options.after.unwrap_or(0);
        for event in &result.events {
            identity(&event.launch_id, id)?;
            if event.seq <= cursor {
                return Err(crate::Error::protocol(
                    "Launch events are not strictly ordered",
                ));
            }
            cursor = event.seq;
        }
        if result.next_cursor != cursor {
            return Err(crate::Error::protocol(
                "Launch event cursor did not match its page",
            ));
        }
        Ok(result)
    }
    /// Create a hosted connection handoff without following its external URL.
    pub async fn create_connection(
        &self,
        body: &CreateConnectionRequest,
        key: &IdempotencyKey,
    ) -> Result<CreatedConnection> {
        self.client
            .command(
                Method::POST,
                &["service-connections"],
                Some(body),
                key,
                StatusCode::CREATED,
            )
            .await
    }
    /// Read one owner-only handoff status.
    pub async fn read_connection(&self, id: &str) -> Result<ConnectionStatus> {
        let result: ConnectionStatus = self.client.get(&["service-connections", id], &[]).await?;
        identity(&result.handoff_id, id)?;
        Ok(result)
    }
}

impl Client {
    /// Access explicit Launch admission, progress, and connection handoffs.
    pub fn launches(&self) -> Launches<'_> {
        Launches { client: self }
    }
}
