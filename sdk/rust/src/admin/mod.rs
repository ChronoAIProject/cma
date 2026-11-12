//! Public operator endpoints using capability checks and settings revision CAS.

use crate::{
    Client, Error, EventOptions, PageOptions, ResourcePage, Result,
    instances::{SandboxEvent, SandboxStatus},
    profiles::{ProfileKind, ProfileVisibility},
    resources::identity,
};
use reqwest::{Method, StatusCode};
mod settings;
mod types;
mod warm_pools;
pub use settings::*;
pub use types::*;
pub use warm_pools::*;

/// Operator Sandbox filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxListOptions {
    /// Owning subject filter.
    pub owner: Option<String>,
    /// Lifecycle filter.
    pub status: Option<SandboxStatus>,
    /// Pinned runtime version filter.
    pub runtime_version: Option<String>,
    /// True selects explicit exceptions; false includes unset preferences.
    pub always_on: Option<bool>,
    /// Opaque pagination.
    pub page: PageOptions,
}

/// Operator profile metadata filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileListOptions {
    /// Profile kind filter.
    pub kind: Option<ProfileKind>,
    /// Visibility filter.
    pub visibility: Option<ProfileVisibility>,
    /// Owning subject filter.
    pub owner: Option<String>,
    /// Opaque pagination.
    pub page: PageOptions,
}

/// Safe activity log filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityListOptions {
    /// Acting subject filter.
    pub actor: Option<String>,
    /// Action filter.
    pub action: Option<ActivityAction>,
    /// Opaque pagination.
    pub page: PageOptions,
}

/// Public operator operations; writes use the route's own CAS or lifecycle rules.
pub struct Administration<'a> {
    client: &'a Client,
}

impl Administration<'_> {
    /// Approve or revoke an exception with revision CAS and an administrator audit record.
    pub async fn set_sandbox_always_on(
        &self,
        id: &str,
        body: &AlwaysOnRequest,
    ) -> Result<crate::instances::Sandbox> {
        let value: crate::instances::Sandbox = self
            .client
            .operation(
                Method::POST,
                &["admin", "sandboxes", id, "always-on"],
                Some(body),
                StatusCode::OK,
            )
            .await?;
        identity(&value.id, id)?;
        Ok(value)
    }

    /// Preview how many Sandboxes satisfy the current idle-pause proof.
    pub async fn preview_auto_pause(&self) -> Result<AutoPausePreview> {
        self.client
            .get(&["admin", "auto-pause", "preview"], &[])
            .await
    }

    /// Schedule currently idle Sandboxes for pause after explicit confirmation.
    pub async fn pause_idle_now(&self, body: &PauseIdleNowRequest) -> Result<PauseIdleNow> {
        self.client
            .operation(
                Method::POST,
                &["admin", "auto-pause", "pause-idle-now"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }

    /// List safe operator Sandbox summaries.
    pub async fn list_sandboxes(
        &self,
        options: &SandboxListOptions,
    ) -> Result<ResourcePage<SandboxSummary>> {
        bound(&options.page)?;
        let mut query = options.page.query();
        if let Some(owner) = &options.owner {
            query.push(("owner", owner.clone()));
        }
        if let Some(status) = options.status {
            query.push(("status", status.as_str().into()));
        }
        if let Some(version) = &options.runtime_version {
            query.push(("runtime_version", version.clone()));
        }
        if let Some(always_on) = options.always_on {
            query.push(("always_on", always_on.to_string()));
        }
        self.client.get(&["admin", "sandboxes"], &query).await
    }
    /// Read one safe operator Sandbox detail.
    pub async fn read_sandbox(&self, id: &str) -> Result<SandboxDetail> {
        let result: SandboxDetail = self.client.get(&["admin", "sandboxes", id], &[]).await?;
        identity(&result.sandbox.id, id)?;
        Ok(result)
    }
    /// Read durable Sandbox events, validating identity and strict ordering.
    pub async fn sandbox_events(
        &self,
        id: &str,
        options: EventOptions,
    ) -> Result<Vec<SandboxEvent>> {
        bound(&PageOptions {
            limit: options.limit,
            cursor: None,
        })?;
        let events: Vec<SandboxEvent> = self
            .client
            .get(&["admin", "sandboxes", id, "events"], &options.query())
            .await?;
        let mut cursor = options.after.unwrap_or(0);
        for event in &events {
            identity(&event.sandbox_id, id)?;
            if event.seq <= cursor {
                return Err(Error::protocol("Sandbox events are not strictly ordered"));
            }
            cursor = event.seq;
        }
        Ok(events)
    }
    /// Retire any Sandbox when the caller has the operator control capability.
    pub async fn retire_sandbox(&self, id: &str) -> Result<Sandbox> {
        let result: Sandbox = self
            .client
            .operation(
                Method::POST,
                &["admin", "sandboxes", id, "retire"],
                None::<&()>,
                StatusCode::OK,
            )
            .await?;
        identity(&result.id, id)?;
        Ok(result)
    }
    /// Queue the managed runtime update across owners after existing work finishes.
    pub async fn update_sandbox_runtime(
        &self,
        id: &str,
        body: &crate::instances::UpdateSandboxRuntimeRequest,
    ) -> Result<crate::instances::Sandbox> {
        let result: crate::instances::Sandbox = self
            .client
            .operation(
                Method::POST,
                &["admin", "sandboxes", id, "runtime-update"],
                Some(body),
                StatusCode::ACCEPTED,
            )
            .await?;
        identity(&result.id, id)?;
        if result.runtime_update.is_none() {
            return Err(Error::protocol(
                "Runtime update response omitted its status",
            ));
        }
        Ok(result)
    }
    /// List profile metadata without private content.
    pub async fn list_profiles(
        &self,
        options: &ProfileListOptions,
    ) -> Result<ResourcePage<ProfileSummary>> {
        bound(&options.page)?;
        let mut query = options.page.query();
        if let Some(kind) = options.kind {
            query.push(("kind", kind.as_str().into()));
        }
        if let Some(visibility) = options.visibility {
            query.push(("visibility", visibility.as_str().into()));
        }
        if let Some(owner) = &options.owner {
            query.push(("owner", owner.clone()));
        }
        self.client.get(&["admin", "profiles"], &query).await
    }
    /// Retire one exact profile kind and identity.
    pub async fn retire_profile(&self, kind: ProfileKind, id: &str) -> Result<ProfileSummary> {
        let result: ProfileSummary = self
            .client
            .operation(
                Method::POST,
                &["admin", "profiles", kind.as_str(), id, "retire"],
                None::<&()>,
                StatusCode::OK,
            )
            .await?;
        identity(&result.id, id)?;
        if result.kind != kind {
            return Err(Error::protocol("profile kind did not match the request"));
        }
        Ok(result)
    }
    /// Read complete settings and current revision.
    pub async fn read_settings(&self) -> Result<StoredSettings> {
        self.client.get(&["admin", "settings"], &[]).await
    }
    /// Replace complete settings at an exact revision, preserving additive fields.
    pub async fn replace_settings(&self, body: &ReplaceSettingsRequest) -> Result<StoredSettings> {
        body.settings.validate()?;
        self.client
            .operation(
                Method::PUT,
                &["admin", "settings"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
    /// Replace only the explicitly supplied settings sections.
    pub async fn patch_settings(&self, body: &PatchSettingsRequest) -> Result<StoredSettings> {
        body.validate()?;
        self.client
            .operation(
                Method::PATCH,
                &["admin", "settings"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
    /// Restore deployment settings at an exact revision.
    pub async fn reset_settings(&self, expected_revision: u64) -> Result<StoredSettings> {
        self.client
            .operation(
                Method::POST,
                &["admin", "settings", "reset"],
                Some(&serde_json::json!({"expected_revision":expected_revision})),
                StatusCode::OK,
            )
            .await
    }
    /// Pause Trigger orchestration without silently disabling admission.
    pub async fn pause_triggers(&self) -> Result<StoredSettings> {
        self.client
            .operation(
                Method::POST,
                &["admin", "triggers", "pause"],
                None::<&()>,
                StatusCode::OK,
            )
            .await
    }
    /// Resume paused Trigger orchestration.
    pub async fn resume_triggers(&self) -> Result<StoredSettings> {
        self.client
            .operation(
                Method::POST,
                &["admin", "triggers", "resume"],
                None::<&()>,
                StatusCode::OK,
            )
            .await
    }
    /// Read observed runtime inventory.
    pub async fn read_runtime_inventory(&self) -> Result<RuntimeInventory> {
        self.client.get(&["admin", "runtime-inventory"], &[]).await
    }
    /// Page through safe operator activity.
    pub async fn list_activity(
        &self,
        options: &ActivityListOptions,
    ) -> Result<ResourcePage<ActivityRecord>> {
        bound(&options.page)?;
        let mut query = options.page.query();
        if let Some(actor) = &options.actor {
            query.push(("actor", actor.clone()));
        }
        if let Some(action) = options.action {
            query.push(("action", action.as_str().into()));
        }
        self.client.get(&["admin", "activity"], &query).await
    }
}

fn bound(options: &PageOptions) -> Result<()> {
    if options
        .limit
        .is_some_and(|value| !(1..=200).contains(&value))
        || options.cursor.as_deref() == Some("")
    {
        return Err(Error::validation(
            "operator page limit must be 1 to 200 and cursor must be nonempty",
        ));
    }
    Ok(())
}

impl Client {
    /// Access public operator inventory and platform settings.
    pub fn admin(&self) -> Administration<'_> {
        Administration { client: self }
    }
}
