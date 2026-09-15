//! Explicit retained-Agent consent management. Requests use actual dashboard
//! authority; durable grant projections cannot authorize a deferred effect.
mod types;
mod validation;
pub use types::*;

use super::{Bridges, validation as bridge_validation};
use crate::{Error, IdempotencyKey, PageOptions, Result, resources::identity};
use reqwest::{Method, StatusCode};

fn key(value: &IdempotencyKey) -> Result<()> {
    if value.as_str().len() > 200 {
        return Err(Error::validation(
            "bridge invoke idempotency key exceeds 200 bytes",
        ));
    }
    Ok(())
}

impl Bridges<'_> {
    /// List current authorized grant history, including revoked entries. The
    /// default page size is 50 and maximum is 100; keep the opaque next cursor.
    pub async fn list_invoke_grants(
        &self,
        bridge_id: &str,
        options: &PageOptions,
    ) -> Result<BridgeInvokePage> {
        bridge_validation::id(bridge_id)?;
        if options.limit.is_some_and(|limit| limit == 0 || limit > 100) {
            return Err(Error::validation(
                "bridge invoke page limit must be 1 through 100",
            ));
        }
        if let Some(cursor) = &options.cursor {
            validation::id(cursor)?;
        }
        let page: BridgeInvokePage = self
            .client
            .get(&["bridges", bridge_id, "invoke-grants"], &options.query())
            .await?;
        if page.items.len() > usize::from(options.limit.unwrap_or(50)) {
            return Err(Error::protocol(
                "bridge invoke page exceeded its requested limit",
            ));
        }
        let mut previous = options.cursor.as_deref();
        let mut owner = None;
        for grant in &page.items {
            grant.check()?;
            if grant.pin.bridge_id != bridge_id
                || previous.is_some_and(|id| id >= grant.id.as_str())
                || owner.is_some_and(|owner| owner != &grant.owner)
            {
                return Err(Error::protocol(
                    "bridge invoke page changed bridge/owner or failed to advance",
                ));
            }
            previous = Some(&grant.id);
            owner = Some(&grant.owner);
        }
        if page
            .next_cursor
            .as_deref()
            .is_some_and(|next| Some(next) != previous || page.items.is_empty())
        {
            return Err(Error::protocol(
                "bridge invoke continuation does not match its page",
            ));
        }
        Ok(page)
    }
    /// Read one retained grant under current bridge-management authority.
    pub async fn read_invoke_grant(&self, grant_id: &str) -> Result<BridgeInvokeGrant> {
        validation::id(grant_id)?;
        let grant: BridgeInvokeGrant = self
            .client
            .get(&["bridge-invoke-grants", grant_id], &[])
            .await?;
        grant.check()?;
        identity(&grant.id, grant_id)?;
        Ok(grant)
    }
    /// Inspect current consent availability. ConsentPresent does not establish
    /// live custody, enablement, allocation readiness or execution permission.
    pub async fn invoke_readiness(&self, bridge_id: &str) -> Result<BridgeInvokeReadiness> {
        bridge_validation::id(bridge_id)?;
        let value: BridgeInvokeReadiness = self
            .client
            .get(&["bridges", bridge_id, "invoke-readiness"], &[])
            .await?;
        value.check(bridge_id)?;
        Ok(value)
    }
    /// Issue explicit consent for the exact retained target. Preserve the same
    /// request and idempotency key if a response is lost or its future cancelled.
    pub async fn issue_invoke_grant(
        &self,
        bridge_id: &str,
        body: &IssueBridgeInvokeGrantRequest,
        idempotency: &IdempotencyKey,
    ) -> Result<BridgeInvokeReceipt> {
        bridge_validation::id(bridge_id)?;
        bridge_validation::revision(body.expected_bridge_revision)?;
        key(idempotency)?;
        if !validation::target(&body.target) {
            return Err(Error::validation("invalid exact bridge invoke target"));
        }
        let receipt: BridgeInvokeReceipt = self
            .client
            .command(
                Method::POST,
                &["bridges", bridge_id, "invoke-grants"],
                Some(body),
                idempotency,
                StatusCode::CREATED,
            )
            .await?;
        receipt.check()?;
        if receipt.bridge_id != bridge_id
            || receipt.result.target.ids != body.target
            || receipt.result.consent != body.shared_context
            || receipt.result.state != BridgeInvokeGrantState::Active
            || receipt.result.pin.config_revision > body.expected_bridge_revision
        {
            return Err(Error::protocol(
                "issued invoke grant does not match its requested bridge, target or consent",
            ));
        }
        Ok(receipt)
    }
    /// Permanently revoke one grant with exact observed revisions. Cancellation
    /// stops awaiting the response; it does not roll back a server-side commit.
    pub async fn revoke_invoke_grant(
        &self,
        grant_id: &str,
        body: &RevokeBridgeInvokeGrantRequest,
        idempotency: &IdempotencyKey,
    ) -> Result<BridgeInvokeReceipt> {
        validation::id(grant_id)?;
        bridge_validation::revision(body.expected_bridge_revision)?;
        bridge_validation::revision(body.expected_revision)?;
        key(idempotency)?;
        let receipt: BridgeInvokeReceipt = self
            .client
            .command(
                Method::POST,
                &["bridge-invoke-grants", grant_id, "revoke"],
                Some(body),
                idempotency,
                StatusCode::OK,
            )
            .await?;
        receipt.check()?;
        if receipt.result.id != grant_id
            || !matches!(receipt.result.state, BridgeInvokeGrantState::Revoked { .. })
            || body.expected_revision.checked_add(1) != Some(receipt.result.revision)
            || receipt.result.pin.config_revision > body.expected_bridge_revision
        {
            return Err(Error::protocol(
                "revoked invoke grant does not match its requested identity or revision",
            ));
        }
        Ok(receipt)
    }
}
