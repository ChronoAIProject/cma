//! Versioned CMA configurations used to provision event workers.
use crate::{Client, IdempotencyKey, Result, bots::BotProfileRef, http::Request};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Published CMA revisions composed into one complete worker configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventWorkerProfileInput {
    /// Owner-facing name.
    pub name: String,
    /// Description of the worker's purpose.
    pub description: String,
    /// Agent instructions and model selection.
    pub agent_profile: BotProfileRef,
    /// Repository and workspace setup.
    pub workspace_profile: BotProfileRef,
    /// Sandbox environment and resource configuration.
    pub sandbox_profile: BotProfileRef,
}
/// One immutable worker revision, including its complete gateway snapshot.
#[derive(Clone, Serialize, Deserialize)]
pub struct EventWorkerProfileRecord {
    /// Stable worker profile identifier.
    pub id: String,
    /// Owning subject.
    pub owner: String,
    /// Immutable revision number.
    pub revision: u32,
    /// Original authoring input.
    pub source: EventWorkerProfileInput,
    /// Complete neutral gateway configuration.
    pub profile: crate::cmaeg::types::EventWorkerProfile,
    /// SHA-256 digest of the complete configuration.
    pub digest: String,
    /// Initial creation time.
    pub created_at: String,
    /// Revision creation time.
    pub updated_at: String,
    /// Retirement time, when no longer offered for new Bots.
    pub retired_at: Option<String>,
}
/// A page of active owned profiles.
#[derive(Clone, Serialize, Deserialize)]
pub struct EventWorkerProfilePage {
    /// Profiles ordered by identifier.
    pub items: Vec<EventWorkerProfileRecord>,
    /// Exclusive continuation cursor.
    pub next_after: Option<String>,
}
/// Optional listing cursor and page size.
#[derive(Debug, Clone, Default)]
pub struct EventWorkerProfileListOptions {
    /// Exclusive continuation cursor.
    pub after: Option<String>,
    /// Page size between one and two hundred.
    pub limit: Option<u32>,
}
/// Revision-fenced update request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateEventWorkerProfile {
    /// Last observed current revision.
    pub expected_revision: u32,
    /// Configuration for the next immutable revision.
    pub source: EventWorkerProfileInput,
}
/// Owner-scoped Event Worker Profile operations.
pub struct EventWorkerProfiles<'a>(&'a Client);
impl Client {
    /// Manage complete event worker configurations through CMA's public API.
    pub fn event_worker_profiles(&self) -> EventWorkerProfiles<'_> {
        EventWorkerProfiles(self)
    }
}
impl EventWorkerProfiles<'_> {
    /// List active profiles owned by the caller.
    pub async fn list(
        &self,
        options: &EventWorkerProfileListOptions,
    ) -> Result<EventWorkerProfilePage> {
        let mut query = Vec::new();
        if let Some(after) = &options.after {
            query.push(("after", after.clone()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        self.0.get(&["event-worker-profiles"], &query).await
    }
    /// Read the current revision, including retired profiles.
    pub async fn get(&self, id: &str) -> Result<EventWorkerProfileRecord> {
        self.0.get(&["event-worker-profiles", id], &[]).await
    }
    /// Read an immutable saved revision.
    pub async fn revision(&self, id: &str, revision: u32) -> Result<EventWorkerProfileRecord> {
        self.0
            .get(
                &[
                    "event-worker-profiles",
                    id,
                    "revisions",
                    &revision.to_string(),
                ],
                &[],
            )
            .await
    }
    /// Create a profile; uncertain retries must retain both input and key.
    pub async fn create(
        &self,
        input: &EventWorkerProfileInput,
        key: &IdempotencyKey,
    ) -> Result<EventWorkerProfileRecord> {
        self.0
            .command(
                Method::POST,
                &["event-worker-profiles"],
                Some(input),
                key,
                StatusCode::CREATED,
            )
            .await
    }
    /// Save the next immutable revision using the previously observed revision.
    pub async fn update(
        &self,
        id: &str,
        input: &UpdateEventWorkerProfile,
    ) -> Result<EventWorkerProfileRecord> {
        self.0
            .operation(
                Method::PUT,
                &["event-worker-profiles", id],
                Some(input),
                StatusCode::OK,
            )
            .await
    }
    /// Retire a profile while preserving historical revisions and existing Bots.
    pub async fn retire(
        &self,
        id: &str,
        expected_revision: u32,
    ) -> Result<EventWorkerProfileRecord> {
        let segments = ["event-worker-profiles", id];
        let query = [("expected_revision", expected_revision.to_string())];
        let mut request =
            Request::operation(Method::DELETE, &segments, None::<&()>, StatusCode::OK)?;
        request.query = &query;
        self.0.request(request).await
    }
}
