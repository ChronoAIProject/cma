use super::*;
use futures_util::TryStreamExt;
use layer2_core::profiles::*;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{ServiceId, SubjectId, WorkspaceProfileId};
use layer3_adapter::agent_sandbox::*;
use layer3_adapter::{
    HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeScope, RuntimeImage,
};

mod adoption_tests;
mod agent_replay_schema_tests;
pub(in crate::store) mod agent_replay_tests;
mod claim_race_tests;
mod guidance_revision_tests;
mod installation_transfer_tests;
pub(super) mod machine_tests;
mod model_tests;
pub(crate) mod open_ready_fixture;
mod runtime_release_tests;
mod target_tests;
mod tool_enrollment_tests;
mod workspace_proof_tests;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";
const EXPIRED: &str = "2026-09-05T00:30:01Z";

fn lease_deadline() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
}

pub(in crate::store) fn plan() -> NativePreparedPlan {
    let sandbox = crate::store::agent_access_fixtures::sandbox("prepared")
        .profile
        .content;
    NativePreparedPlan::new(
        sandbox,
        WorkspaceProfileContent {
            location: WorkspaceLocation::Directory {
                name: "workspace".into(),
            },
            name: "workspace".into(),
            setup_commands: vec![],
            agent_profiles: vec![],
        },
        crate::agent_lifecycle::prepared_default_agent_profile().unwrap(),
        vec![],
        format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
        Default::default(),
        HomeVolumePolicy {
            mode: HomeVolumeMode::NativeVctFullHomeV2,
            storage_class: "standard-rwo".into(),
            capacity_gib: 8,
            template_name: String::new(),
            template_generation: String::new(),
        },
    )
    .unwrap()
}

fn actual_profiles(
    plan: &NativePreparedPlan,
) -> (SandboxProfileSnapshot, WorkspaceProfileSnapshot) {
    let mut workspace = plan.warm.workspaces[0].profile.clone();
    workspace.id = WorkspaceProfileId("wsp_private_actual".into());
    workspace.content.agent_profiles.clear();
    workspace.digest = content_digest(&workspace.content).unwrap();
    let mut sandbox = plan.warm.profile.clone();
    sandbox.id = layer2_core::SandboxProfileId("sbp_private_actual".into());
    sandbox.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        workspace.id.clone(),
        workspace.revision,
    )];
    sandbox.digest = content_digest(&sandbox.content).unwrap();
    (sandbox, workspace)
}

fn ready(mut record: Record) -> Record {
    let warm = &mut record.payload;
    let mut home = SandboxHome::reserve(&warm.id, record.plan.home_policy.clone()).unwrap();
    home.request.scope = Some(
        HomeVolumeScope::from_authority(
            "native-vct-empty-v1",
            &["open-ready-test", "default", "open-ready"],
        )
        .unwrap(),
    );
    let request = NativePoolClaimRequest {
        pool: NativePoolBinding {
            spec: NativePoolSpec {
                cold_canary: false,
                image: RuntimeImage {
                    uri: record.plan.warm.runtime_version.clone(),
                    labels: std::collections::BTreeMap::from([
                        (layer3_adapter::RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                        ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                        ("dev.chrono.cma.home-layout".into(), "2".into()),
                    ]),
                },
                cpu_millis: 1000,
                memory_mib: 2048,
                home: record.plan.home_policy.clone(),
            },
            pod_policy: NativePoolPodPolicy {
                runtime_class: None,
                service_account: "runtime".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                fs_group_change_policy: None,
            },
            name: "pool".into(),
            uid: "pool-uid".into(),
            template_name: "template".into(),
            template_uid: "template-uid".into(),
            config_sha256: "a".repeat(64),
        },
        home: home.request.clone(),
        runtime_generation: 1,
    };
    let claim = NativePoolClaim {
        request: request.clone(),
        name: "claim".into(),
        uid: "claim-uid".into(),
        resource_version: "1".into(),
        sandbox_name: Some("member".into()),
    };
    home.native_pool = Some(Box::new(SandboxNativePool {
        request,
        claim: Some(claim.clone()),
        cleanup: None,
        cleaned: false,
    }));
    home.bind(HomeVolumeReceipt {
        native_authority: Some(Box::new(NativePoolHomeOrigin {
            claim,
            sandbox_uid: "sandbox-uid".into(),
            pvc_uid: "pvc-uid".into(),
        })),
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-member".into(),
        storage_generation: "pvc-uid".into(),
    })
    .unwrap();
    warm.home = Some(Box::new(home));
    warm.runtime = Some(RuntimeBinding {
        runtime_id: format!("runtime-{}", warm.id),
        runtime_version: record.plan.warm.runtime_version.clone(),
        provision_generation: Some(1),
        boot_id: Some("b".repeat(32)),
        bound_at: NOW.into(),
    });
    warm.state = WarmSandboxState::Ready;
    warm.installed_request_sha256 = Some(
        layer3_adapter::InstallRequest::with_timeouts(
            format!("sandbox-install-v1:{}", warm.id),
            warm.plan.profile.content.software_install_commands.clone(),
            warm.plan.execution.command_timeout_seconds,
            warm.plan.execution.total_timeout_seconds,
            layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        )
        .request_sha256,
    );
    warm.workspaces[0].resolved_commit = Some("a".repeat(40));
    warm.workspaces[0].setup_ready = true;
    warm.workspaces[0].agents[0].skills = Some(vec![]);
    warm.workspaces[0].agents[0].conversation_id = Some("prepared-session".into());
    record.validate().unwrap();
    record
}

async fn database() -> (mongodb::Client, String, MongoSandboxStore) {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("requires local disposable replica set");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_native_prepared_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    (client, name, store)
}
async fn records(store: &MongoSandboxStore) -> Vec<Record> {
    store
        .native_prepared
        .find(doc! {"kind":"reservation"})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .iter()
        .map(|r| read(r).unwrap())
        .collect()
}
async fn publish_ready(store: &MongoSandboxStore) -> Record {
    let old = records(store)
        .await
        .into_iter()
        .find(|r| r.payload.state == WarmSandboxState::Preparing)
        .unwrap();
    let record = ready(old);
    let mut value = document(&record).unwrap();
    value.insert("lease_until", NOW);
    value.insert("lease_token", mongodb::bson::Bson::Null);
    value.insert("check_after", LATER);
    store
        .native_prepared
        .replace_one(doc! {"_id":record.payload.id.as_str()}, value)
        .await
        .unwrap();
    record
}

async fn admission(
    store: &MongoSandboxStore,
    plan: &NativePreparedPlan,
    owner: &str,
    key: &str,
) -> SandboxAdmission {
    let database = store.client.database(&store.sandboxes.namespace().db);
    let ops = crate::store::MongoAgentLifecycleStore::open(&database)
        .await
        .unwrap();
    let result = ops
        .create(&SubjectId::new(owner), key, lifecycle_input(plan))
        .await
        .unwrap();
    let crate::store::CreateResult::Created(operation_id) = result else {
        panic!("new operation")
    };
    sandbox_admission(store, plan, owner, key, operation_id).await
}

fn lifecycle_input(plan: &NativePreparedPlan) -> crate::store::LifecycleInput {
    crate::store::LifecycleInput {
        behavior: crate::store::LifecycleBehavior::PromptRepository,
        managed_agent_instructions: Some(
            plan.warm.workspaces[0].agents[0]
                .content
                .system_prompt
                .clone(),
        ),
        setup_message: None,
        request: serde_json::from_value(
            serde_json::json!({"name":"Prepared test", "machine_included":plan.machine()}),
        )
        .unwrap(),
        resolved_image: plan.machine().then(|| plan.warm.runtime_version.clone()),
        capacity: Some(layer3_adapter::SandboxCapacity {
            cpu_millis: 1000,
            memory_mib: 2048,
        }),
        sandbox_availability: Default::default(),
        capabilities: vec![],
        actor: None,
    }
}

async fn sandbox_admission(
    store: &MongoSandboxStore,
    plan: &NativePreparedPlan,
    owner: &str,
    key: &str,
    operation_id: String,
) -> SandboxAdmission {
    let database = store.client.database(&store.sandboxes.namespace().db);
    let (profile, workspace) = actual_profiles(plan);
    let revision = WorkspaceProfileRevision {
        profile_id: workspace.id.clone(),
        revision: workspace.revision,
        content: workspace.content,
        digest: workspace.digest,
        published_at: NOW.into(),
    };
    database
        .collection::<Document>(crate::store::sandbox_mongo::WORKSPACE_PROFILE_REVISIONS)
        .replace_one(
            doc! {"_id":format!("{}:{:010}",revision.profile_id,revision.revision)},
            crate::store::profile_documents::revision_document(
                revision.profile_id.as_str(),
                revision.revision,
                &revision,
            )
            .unwrap(),
        )
        .upsert(true)
        .await
        .unwrap();
    let mut parent =
        crate::store::sandbox_admission_mongo_integration_tests::sandbox("sbx_cold", key);
    parent.name = "prepared".into();
    parent.owner = SubjectId::new(owner);
    parent.origin = SandboxOrigin::Lifecycle { operation_id };
    parent.profile = profile;
    parent.credential.reaches = plan.warm.reaches.clone();
    parent.credential.sign_in = SandboxSignInState::NotRequired;
    parent.limits = SandboxLimits {
        max_workspaces: 4,
        max_agents_per_workspace: 4,
    };
    SandboxAdmission::new(
        parent,
        SandboxAdmissionName::Allocate("prepared".into()),
        20,
        NOW.into(),
    )
    .unwrap()
}

async fn admit_at(
    store: &MongoSandboxStore,
    admission: SandboxAdmission,
    observed_at: &str,
) -> Result<SandboxAdmissionResult, SandboxStoreError> {
    store
        .commit_sandbox_admission_observing(admission, || observed_at.to_owned())
        .await
}

async fn exact_claim_authority(
    store: &MongoSandboxStore,
    request: &NativePoolClaimRequest,
) -> bool {
    store
        .unowned_home_reservation(&SandboxId(request.home.sandbox_id.clone()))
        .await
        .unwrap()
        .is_some_and(|home| {
            !home.released
                && home
                    .native_pool
                    .as_ref()
                    .is_some_and(|native| !native.cleaned && native.request == *request)
        })
}
