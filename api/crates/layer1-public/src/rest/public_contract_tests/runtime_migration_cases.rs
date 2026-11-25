//! Operator contracts use a real retained-home admission and its exact HTTP replay.
use super::{Corpus, Scenario};
use crate::rest::admin::{runtime_migration::CustomRuntimeMigrationRequest, tests};
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::{
    RuntimeActivityVerifier, RuntimeMigrationIdentity, Sandbox,
    SandboxCustomRuntimeMigrationService, SandboxCustomRuntimeOrigin, SandboxHome,
    SandboxModelError, SandboxNativePool,
};
use layer2_core::{Authorization, Capability, SandboxId};
use std::sync::Arc;

const NOW: &str = "2026-09-05T00:00:00Z";

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let (activity, mode) = if let Some(mode) = name.strip_prefix("admin_runtime_activity_") {
        (true, mode)
    } else {
        (false, name.strip_prefix("admin_runtime_migration_")?)
    };
    let verifier = (activity && mode == "unavailable").then(unavailable_verifier);
    let f = if !activity && mode == "unavailable" {
        tests::fixture(false)
    } else {
        tests::fixture_with_configure(false, None, |state, store| {
            state.with_runtime_migrations(
                Arc::new(SandboxCustomRuntimeMigrationService::new(
                    store.clone(),
                    store.clone(),
                )),
                verifier,
            )
        })
    };
    if !activity && mode != "missing" {
        let corpus: Corpus = serde_json::from_str(include_str!(
            "../../../tests/fixtures/public-contract/admin-runtime-migration.json"
        ))
        .unwrap();
        let command: CustomRuntimeMigrationRequest =
            serde_json::from_value(corpus.cases[0].request.body.clone().unwrap()).unwrap();
        let original = paused(&command);
        let identity = RuntimeMigrationIdentity::capture(&original).unwrap();
        f.sandboxes.create_sandbox(original).await.unwrap();
        if mode != "closed" {
            let now = runtime_migration_now_ms().unwrap();
            // This test's server-owned basis enters the supported atomic store
            // port. Generic create/save never installs custom migration authority.
            f.sandboxes
                .admit_custom_runtime_migration(SandboxCustomRuntimeMigrationAdmission {
                    migration: command.migration.clone(),
                    expected_current_revision: command.expected_current_revision,
                    restart_failed: false,
                    basis: RuntimeMigrationAdmissionBasis {
                        authority_id: command.authority_id,
                        policy_id: "contract-policy".into(),
                        request_key: command.migration.request_key,
                        source_runtime: identity.runtime.clone(),
                        identity,
                        authority_issued_at_ms: now,
                        authority_expires_at_ms: now + 30_000,
                        approved_target: command.migration.target_image,
                        clearance: RuntimeMigrationClearance::Paused,
                        admitted_at: layer2_core::now_rfc3339(),
                        previous_request_keys: vec![],
                        earlier_admissions: vec![],
                    },
                })
                .await
                .unwrap();
        }
    }
    Some(Scenario {
        router: f.app,
        subject: (mode != "anonymous").then_some("operator-a"),
        authorization: Authorization::new([match mode {
            "ordinary" => Capability::UseOwnSandboxes,
            "wrong_capability" if activity => Capability::ControlAnySandbox,
            "wrong_capability" => Capability::ReadAllSandboxes,
            _ if activity => Capability::ReadAllSandboxes,
            _ => Capability::ControlAnySandbox,
        }]),
        symbols: Default::default(),
    })
}

fn paused(command: &CustomRuntimeMigrationRequest) -> Sandbox {
    let migration = &command.migration;
    let mut sandbox = tests::sandbox("sbx_route", "foreign-owner", &migration.expected_image);
    sandbox.profile.content.runtime_image = Some(migration.expected_image.clone());
    sandbox.profile.digest =
        layer2_core::profiles::content_digest(&sandbox.profile.content).unwrap();
    let attachment = &migration.origin.home;
    let claim = &attachment.receipt.native_authority.as_ref().unwrap().claim;
    sandbox.lifecycle.home = Some(Box::new(SandboxHome {
        request: attachment.request.clone(),
        receipt: Some(attachment.receipt.clone()),
        released: false,
        native_pool: Some(Box::new(SandboxNativePool {
            request: claim.request.clone(),
            claim: Some(claim.clone()),
            cleanup: None,
            cleaned: false,
        })),
    }));
    let binding = sandbox.runtime.as_mut().unwrap();
    binding.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    binding.boot_id = Some("b".repeat(32));
    sandbox.validate().unwrap();
    sandbox.begin_pause(NOW.into(), None).unwrap();
    for transition in [
        Sandbox::complete_lifecycle_persistent_pause,
        Sandbox::complete_lifecycle_agent_pause,
        Sandbox::complete_lifecycle_runtime_suspension,
        Sandbox::publish_lifecycle_paused,
    ] {
        pause_step(&mut sandbox, transition);
    }
    sandbox.revision = 1;
    sandbox.validate().unwrap();
    assert_eq!(
        SandboxCustomRuntimeOrigin::capture(&sandbox).unwrap(),
        migration.origin
    );
    assert_eq!(sandbox.revision, command.expected_current_revision);
    sandbox
}

fn pause_step(
    sandbox: &mut Sandbox,
    transition: fn(&mut Sandbox, &str, u64, String) -> Result<(), SandboxModelError>,
) {
    let fence = sandbox
        .work
        .claim("contract".into(), NOW, "2026-09-05T01:00:00Z".into())
        .unwrap();
    transition(sandbox, "contract", fence, NOW.into()).unwrap();
}

struct UnavailableObservation;
#[async_trait::async_trait]
impl SandboxRuntimeActivityObservationStore for UnavailableObservation {
    async fn observe_runtime_activity(
        &self,
        _: &SandboxId,
        _: &RuntimeActivitySourceContext,
    ) -> Result<RuntimeActivityObservation, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "contract dependency failure".into(),
        ))
    }
}
fn unavailable_verifier() -> Arc<RuntimeActivityVerifier> {
    Arc::new(RuntimeActivityVerifier::new(
        Arc::new(UnavailableObservation),
        Some(RuntimeActivitySourceContext {
            source_id: "contract".into(),
            adapter_version: "fixture-v1".into(),
            policy_id: "contract-policy".into(),
            producer_id: "contract".into(),
            database_id: "memory".into(),
            api_image: "fixture".into(),
            source_revision: "fixture".into(),
            process_incarnation: "fixture".into(),
            serving_processes: 1,
            replica_mode: false,
            clock_uncertainty_ms: 0,
            coverage: RuntimeActivityCoverage::MemoryFixtureV1,
        }),
        vec![SandboxId("sbx_route".into())],
    ))
}
