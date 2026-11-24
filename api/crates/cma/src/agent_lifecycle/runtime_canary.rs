//! Compose verified deployment evidence with the durable owner canary store.
use super::*;
use crate::config::{runtime_canary, RuntimeConfig};
use crate::store::runtime_canary::{
    CanaryLane, CanaryPolicy, CanaryWriteError, FreshCanaryGrant, FreshCanaryRedemption,
    PrepareCanaryGrant, PreparedCanaryRedemption, RevokeCanaryGrant,
};
use layer1_public::rest::runtime_canary::{
    CanaryGrantRequest, CanaryGrantView, CanaryPolicyRequest, CanaryPolicyView,
    CanaryRedeemRequest, CanaryRefusal, CanaryRevokeRequest, RuntimeCanaryService,
};

pub(crate) struct Canaries {
    lifecycle: Arc<InlineAgentLifecycle>,
    catalog: runtime_canary::Catalog,
    scope: runtime_canary::WriteScope,
    catalog_sha256: String,
    owner_scope_sha256: String,
}

impl Canaries {
    pub(crate) fn new(
        config: &RuntimeConfig,
        lifecycle: Arc<InlineAgentLifecycle>,
    ) -> Result<Self, String> {
        let catalog = config.runtime_canary_releases.clone();
        let scope = config.runtime_canary_writes.clone();
        Ok(Self {
            lifecycle,
            catalog_sha256: catalog.fingerprint()?,
            owner_scope_sha256: scope.fingerprint()?,
            catalog,
            scope,
        })
    }
}

#[async_trait]
impl RuntimeCanaryService for Canaries {
    async fn configure(
        &self,
        request: CanaryPolicyRequest,
    ) -> Result<CanaryPolicyView, CanaryRefusal> {
        if request.open && !self.scope.enabled() {
            return Err(CanaryRefusal::Closed);
        }
        let revision = CanaryPolicy::configure(
            &self.lifecycle.operations.canary_database(),
            request.expected_revision,
            request.open,
            self.catalog_sha256.clone(),
            self.owner_scope_sha256.clone(),
        )
        .await
        .map_err(refusal)?;
        tracing::info!(parent: None, open = request.open, "runtime canary policy changed");
        Ok(CanaryPolicyView { revision })
    }

    async fn grant(&self, request: CanaryGrantRequest) -> Result<CanaryGrantView, CanaryRefusal> {
        let fresh = self
            .scope
            .allows(&request.owner)
            .then(|| self.catalog.release(&request.release_key))
            .flatten()
            .map(|release| FreshCanaryGrant {
                lane: lane(release.lane),
                release: release.into(),
                catalog_sha256: self.catalog_sha256.clone(),
                owner_scope_sha256: self.owner_scope_sha256.clone(),
            });
        let expires = mongodb::bson::DateTime::parse_rfc3339_str(&request.expires_at)
            .map_err(|_| CanaryRefusal::Invalid)?;
        let store = &self.lifecycle.operations;
        let grant = store
            .issue_canary_grant(
                &store.canary_database(),
                &PrepareCanaryGrant {
                    owner: request.owner,
                    expected_revision: request.expected_revision,
                    release_key: request.release_key,
                    policy_revision: request.policy_revision,
                    consent_sha256: request.consent_sha256,
                    expires_at: expires,
                    fresh,
                },
            )
            .await
            .map_err(refusal)?;
        tracing::info!(parent: None, "runtime canary grant issued");
        Ok(CanaryGrantView {
            grant_id: grant.id,
            revision: grant.revision,
            policy_revision: grant.policy_revision,
        })
    }

    async fn revoke(&self, request: CanaryRevokeRequest) -> Result<CanaryGrantView, CanaryRefusal> {
        let store = &self.lifecycle.operations;
        let grant = store
            .revoke_canary_grant(
                &store.canary_database(),
                &RevokeCanaryGrant {
                    owner: request.owner,
                    release_key: request.release_key,
                    grant_id: request.grant_id,
                    revision: request.expected_revision,
                },
            )
            .await
            .map_err(refusal)?;
        tracing::info!(parent: None, "runtime canary grant revoked");
        Ok(CanaryGrantView {
            grant_id: grant.id,
            revision: grant.revision,
            policy_revision: grant.policy_revision,
        })
    }

    async fn redeem(
        &self,
        context: &CallerContext,
        key: &str,
        request: CanaryRedeemRequest,
    ) -> Result<AgentLifecycleCreateReceipt, CanaryRefusal> {
        if self.lifecycle.tasks.is_closed() {
            return Err(CanaryRefusal::Unavailable);
        }
        let owner = &context.caller.subject;
        let fresh = self.fresh_redemption(owner, &request).await;
        let lifecycle_request = AgentLifecycleCreateRequest {
            name: request.name,
            title_mode: None,
            initial_message: Some(request.initial_message),
            template: None,
            desktop: false,
            machine_included: false,
            workspace_id: None,
            sandbox_id: None,
            image: None,
            repository: None,
            profile: None,
        };
        validate_request(&lifecycle_request).map_err(|_| CanaryRefusal::Invalid)?;
        let input = LifecycleInput {
            behavior: LifecycleBehavior::PromptRepository,
            managed_agent_instructions: Some(guidance::managed_v3().to_owned()),
            setup_message: Some(validation::setup_message(&lifecycle_request)),
            request: lifecycle_request,
            resolved_image: None,
            capacity: None,
            sandbox_availability: Default::default(),
            capabilities: context.caller.authorization.iter().collect(),
            actor: Some(context.actor.clone()),
        };
        let store = &self.lifecycle.operations;
        let accepted = store
            .redeem_canary(
                &store.canary_database(),
                PreparedCanaryRedemption {
                    owner: owner.clone(),
                    key: key.to_owned(),
                    grant_id: request.grant_id,
                    release_key: request.release_key,
                    policy_revision: request.policy_revision,
                    fresh,
                    input,
                },
            )
            .await
            .map_err(refusal)?;
        let id = match accepted {
            CreateResult::Created(id) => {
                tracing::info!(parent: None, "runtime canary admission committed");
                id
            }
            CreateResult::Existing(row) => {
                store
                    .resume_connection(owner, &row.id)
                    .await
                    .map_err(|_| CanaryRefusal::Unavailable)?;
                row.id
            }
        };
        self.lifecycle.spawn_worker(id.clone());
        Ok(AgentLifecycleCreateReceipt {
            id,
            status: "provisioning".into(),
        })
    }
}

impl Canaries {
    async fn fresh_redemption(
        &self,
        owner: &SubjectId,
        request: &CanaryRedeemRequest,
    ) -> Option<FreshCanaryRedemption> {
        if !self.scope.allows(owner) {
            return None;
        }
        let release = self.catalog.release(&request.release_key)?;
        if release.lane == runtime_canary::Lane::DebianDesktop {
            let desktop = self.lifecycle.desktop.as_ref()?;
            if !desktop
                .canary_capability(&release.image.uri, release.capacity)
                .await
                .available()
            {
                return None;
            }
        }
        Some(FreshCanaryRedemption {
            lane: lane(release.lane),
            policy_revision: request.policy_revision,
            catalog_sha256: self.catalog_sha256.clone(),
            owner_scope_sha256: self.owner_scope_sha256.clone(),
            release: release.into(),
        })
    }
}

fn lane(lane: runtime_canary::Lane) -> CanaryLane {
    match lane {
        runtime_canary::Lane::Agent => CanaryLane::Agent,
        runtime_canary::Lane::DebianDesktop => CanaryLane::DebianDesktop,
    }
}

fn refusal(error: CanaryWriteError) -> CanaryRefusal {
    match error {
        CanaryWriteError::Closed => CanaryRefusal::Closed,
        CanaryWriteError::Refused => CanaryRefusal::Conflict,
        CanaryWriteError::OwnerUnavailable => CanaryRefusal::Forbidden,
        CanaryWriteError::Store(_) | CanaryWriteError::Driver(_) | CanaryWriteError::Unresolved => {
            CanaryRefusal::Unavailable
        }
    }
}
