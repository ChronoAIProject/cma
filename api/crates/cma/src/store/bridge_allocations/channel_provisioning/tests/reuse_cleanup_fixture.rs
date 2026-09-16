use super::*;
use crate::{
    bridge::provisioning::reuse_cleanup::ChannelReuseCleanupAuthority,
    sandbox::channel_reuse_cleanup::{ChannelReuseCleanupRequest, ChannelReuseCleanupState},
};
use layer2_core::resource_access::{AccessPurpose, ResourceAccessService, ResourceCapability};

use super::super::home;

pub(super) struct CleanupFixture {
    pub f: ReuseCommandFixture,
    pub original: AllocationRecord,
}
impl CleanupFixture {
    pub async fn new(shared_workspace: bool) -> Self {
        Self::with_capacity(shared_workspace, 4).await
    }
    pub async fn with_capacity(shared_workspace: bool, capacity: u8) -> Self {
        // Provision a persistent home so the actual runtime status supplies a
        // concrete boot identity before the shared child is created.
        let seed = home::seed().await;
        let mut reuse =
            super::super::reuse::fixture::ReuseFixture::with_original(shared_workspace, seed).await;
        reuse.parent.profile.content.max_workspaces = capacity;
        reuse.parent.profile.content.max_agents_per_workspace = capacity;
        reuse.parent.profile.digest =
            layer2_core::profiles::content_digest(&reuse.parent.profile.content).unwrap();
        reuse.parent.limits = SandboxLimits {
            max_workspaces: capacity,
            max_agents_per_workspace: capacity,
        };
        reuse.parent.validate().unwrap();
        reuse.profiles.sandbox = reuse.parent.profile.clone();
        reuse.profiles.trust_digest = AllocationProfiles::digest(
            &reuse.profiles.sandbox,
            &reuse.profiles.workspace,
            &reuse.profiles.agent,
        )
        .unwrap();
        reuse.original.client.database(&reuse.original.database)
            .collection::<Document>("sandboxes").update_one(
                doc! {"_id":reuse.parent.id.as_str()},
                doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&reuse.parent).unwrap()},
            ).await.unwrap();
        let mut f = ReuseCommandFixture::from_reuse(reuse, false).await;
        // The reusable fixture approves creation as Human but its default
        // runtime lease uses Api. Preserve the exact stored grant and original
        // approved actor when constructing this cleanup authority fixture.
        f.f.original.lease = original_human(&f);
        let original = f.record.clone();
        Self { f, original }
    }
    pub fn db(&self) -> mongodb::Database {
        self.f
            .f
            .original
            .client
            .database(&self.f.f.original.database)
    }
    pub async fn row(&self, name: &str, id: &str) -> Document {
        self.db()
            .collection::<Document>(name)
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn reset(&mut self) -> AllocationRecord {
        let next = self.f.f.reset(&self.f.record, "67890").await.unwrap();
        self.f.record = self
            .f
            .f
            .original
            .allocations
            .generation(&self.f.record.identity)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(self.f.record.state, AllocationState::Retired);
        assert_eq!(
            self.db()
                .collection::<Document>("channel_reuse_cleanup_requests")
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        next
    }
    pub async fn permit(&self) -> ChannelReuseCleanupAuthority {
        let original = &self.f.f.original;
        let bridge = original
            .bridges
            .bridge(&self.f.record.identity.bridge_id)
            .await
            .unwrap()
            .unwrap();
        let management = original
            .svc
            .management_lease(
                original.lease.actor_context(),
                Some(original.lease.credential()),
                &bridge.id,
                bridge.revision,
            )
            .await
            .unwrap();
        let access = ResourceAccessService::new(original.resources.clone(), Arc::new(Authority));
        let parent = access
            .authorize(
                original.lease.actor_context(),
                Some(original.lease.credential()),
                &self.f.record.config.placement.reused_parent().unwrap(),
                ResourceCapability::ManageSandbox,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await
            .unwrap();
        let permit = ChannelReuseCleanupAuthority::fixture(
            self.f.record.clone(),
            management,
            parent,
            original.lease.clone(),
        );
        if self.f.record.state == AllocationState::Retired
            && permit.parent().lineage.sandbox.status == SandboxStatus::Active
        {
            assert!(
                self.f.record.effect_started,
                "fixture has no retained effect start"
            );
            assert!(
                self.f.record.lease.is_none(),
                "fixture retains allocation lease"
            );
            assert_eq!(
                permit.management().actor(),
                &permit.human().actor_context().actor
            );
            assert_eq!(
                permit.management().actor(),
                &self.f.record.placement.profiles().unwrap().approved_by
            );
            let sandbox = &permit.parent().lineage.sandbox;
            assert!(
                sandbox
                    .runtime
                    .as_ref()
                    .unwrap()
                    .provision_generation
                    .is_some(),
                "fixture has no runtime generation"
            );
            assert!(
                sandbox.runtime.as_ref().unwrap().boot_id.is_some(),
                "fixture has no runtime boot identity"
            );
            permit
                .require_current()
                .expect("complete fixture must pass local cleanup authority");
        }
        permit
    }
    pub fn request(&self) -> ChannelReuseCleanupRequest {
        ChannelReuseCleanupRequest {
            request_id: "explicit-cleanup-request".into(),
            allocation_id: self.f.record.id.clone(),
            expected_allocation_revision: self.f.record.revision,
        }
    }
    pub async fn request_cleanup(&self) -> Result<ChannelReuseCleanupState, AllocationError> {
        self.f
            .f
            .original
            .allocations
            .request_channel_reuse_cleanup(&self.permit().await, &self.request())
            .await
    }
    pub async fn assert_no_request(&self) {
        for collection in [
            "channel_reuse_cleanup_requests",
            "channel_reuse_cleanup_audits",
        ] {
            assert_eq!(
                self.db()
                    .collection::<Document>(collection)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0
            );
        }
        assert!(!self
            .row("bridge_allocations", &self.f.record.id)
            .await
            .contains_key("channel_reuse_cleanup_request"));
    }
    pub async fn close(self) {
        self.f.close().await;
    }
}

fn original_human(f: &ReuseCommandFixture) -> Arc<crate::bridge::BridgeControlPlaneLease> {
    use crate::auth::{
        bridge_control_plane::BridgeHumanLease,
        persistence::{DurableSubjectTokens, SecretValue, SubjectAuthState},
        profile::Profile,
    };
    let original = &f.f.original.lease;
    let mut context = original.actor_context().clone();
    context.actor = f.record.placement.profiles().unwrap().approved_by.clone();
    let mut state = SubjectAuthState::new(
        original.grant().subject.clone(),
        crate::permissions::capabilities_from_permissions(["cma:*"]),
        Profile::default(),
        Some(DurableSubjectTokens {
            access_token: SecretValue::new(original.credential().into()),
            refresh_token: Some(SecretValue::new("fixture-refresh".into())),
            expires_at: SystemTime::now() + Duration::from_secs(60),
        }),
    );
    state.incarnation = original.grant().incarnation.clone();
    Arc::new(crate::bridge::BridgeControlPlaneLease::fixture(
        original.grant().clone(),
        BridgeHumanLease::fixture(state, context, original.deadline()),
        original.custody_revision(),
    ))
}
