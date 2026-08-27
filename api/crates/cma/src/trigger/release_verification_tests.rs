//! Deterministic release journey for the complete Trigger orchestration path.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use layer2_core::{
    ActivityService, Authorization, BaselineArchivePlan, BaselineImportBlob, BaselineImportChunk,
    BaselineImportOutcome, BaselineObjectStore, BranchRecord, Caller, Capability,
    CheckpointObjectStore, Confirmation, ConfirmationAction, DiffArtifact, DiffArtifactObjectStore,
    DiffChangeKind, DiffValidationOutcome, EffectClaim, Launch, LaunchEffectKind, LaunchState,
    PromptDelivery, PromptDeliveryState, PublicationState, PullRequestOperation, PullRequestRecord,
    PullRequestState, SessionId, SessionStatus, SubjectId, TriggerService, TriggerStoreError,
    TriggerSupportLookup, WorkspaceCheckpoint, WorkspaceEntry, WorkspaceEntryKind,
    WorkspaceManifest,
};
use sha2::{Digest as _, Sha256};

use super::tests::{
    fork, prompt_delivery, reconciled_pull_request, test_archive, test_signer, trigger_session,
};
use super::*;
use crate::trigger::github::{
    BranchContext, BranchReconciliation, ForkContext, ForkReconciliation, GithubBrokerError,
    PreparedGitData, PublicationAdvance, PublishedGitData, PullRequestAdvance, WorkspaceContext,
    WorkspaceSourcePlan,
};
use crate::trigger::worker::tests::{launch_fixture, service_for_launch, RecordingActivityStore};
use crate::trigger::worker::{EffectRunOutcome, EffectWorker};

const NOW: &str = "2026-08-27T00:00:00Z";
const CREDENTIAL_MARKER: &str = "SENSITIVE_CREDENTIAL_SENTINEL_RELEASE_673";
const PROVIDER_BODY_MARKER: &str = "PRIVATE_PROVIDER_BODY_SENTINEL_RELEASE_673";
const PROMPT_MARKER: &str = "PROMPT_SENTINEL_RELEASE_673";
const PATH_MARKER: &str = "release-fixture/path-sentinel-673.txt";
const CHECKPOINT_CONTENT: &[u8] = b"WORKSPACE_CONTENT_SENTINEL_RELEASE_673\n";

#[derive(Default)]
struct ReleaseGithub {
    preflight_calls: AtomicUsize,
    fork_create_calls: AtomicUsize,
    fork_reconcile_calls: AtomicUsize,
    branch_create_calls: AtomicUsize,
    branch_reconcile_calls: AtomicUsize,
    workspace_import_calls: AtomicUsize,
    capability_preflight_calls: AtomicUsize,
    effect_authority_preflights: Mutex<Vec<LaunchEffectKind>>,
    publication_calls: AtomicUsize,
    publication_object_writes: AtomicUsize,
    publication_ref_checks: AtomicUsize,
    publication_ref_writes: AtomicUsize,
    pull_request_write_calls: AtomicUsize,
    pull_request_reconcile_calls: AtomicUsize,
    publication_answers: Mutex<VecDeque<PublicationAdvance>>,
    pull_request_answer: Mutex<Option<PullRequestRecord>>,
    provider_body_marker: &'static str,
}

impl ReleaseGithub {
    fn new() -> Self {
        let prepared = PreparedGitData {
            base_tree_id: "b".repeat(40),
            tree_id: "c".repeat(40),
            commit_id: "d".repeat(40),
            object_ids: vec!["e".repeat(40)],
        };
        Self {
            publication_answers: Mutex::new(
                [
                    PublicationAdvance::ObjectsWritten(prepared.clone()),
                    PublicationAdvance::RefUpdatePending(prepared.clone()),
                    PublicationAdvance::Published(PublishedGitData {
                        base_tree_id: prepared.base_tree_id,
                        tree_id: prepared.tree_id,
                        commit_id: prepared.commit_id.clone(),
                        resulting_head: prepared.commit_id,
                        object_ids: prepared.object_ids,
                    }),
                ]
                .into(),
            ),
            provider_body_marker: PROVIDER_BODY_MARKER,
            ..Self::default()
        }
    }

    fn set_pull_request_answer(&self, answer: PullRequestRecord) {
        *self.pull_request_answer.lock().unwrap() = Some(answer);
    }
}

#[async_trait]
impl GithubEffectPort for ReleaseGithub {
    async fn preflight(&self, launch: &Launch) -> Result<LaunchAuthority, TriggerServiceError> {
        self.preflight_calls.fetch_add(1, Ordering::SeqCst);
        Ok(LaunchAuthority {
            connection: launch.connection.clone().unwrap(),
            provenance: launch.authority.clone().unwrap(),
            source_sha: launch.source_sha.clone(),
            workflow_paths: launch.workflow_paths.clone(),
        })
    }

    async fn create_fork(
        &self,
        context: &ForkContext,
    ) -> Result<ForkReconciliation, GithubBrokerError> {
        self.fork_create_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ForkReconciliation::Ready(Box::new(fork(context))))
    }

    async fn reconcile_fork(
        &self,
        _context: &ForkContext,
    ) -> Result<ForkReconciliation, GithubBrokerError> {
        self.fork_reconcile_calls.fetch_add(1, Ordering::SeqCst);
        Err(GithubBrokerError::Malformed(
            "the release happy path must not reconcile its fork".into(),
        ))
    }

    async fn create_branch(
        &self,
        context: &BranchContext,
    ) -> Result<BranchReconciliation, GithubBrokerError> {
        self.branch_create_calls.fetch_add(1, Ordering::SeqCst);
        Ok(BranchReconciliation::Ready(BranchRecord {
            name: context.branch_name(),
            expected_head: context.source_sha.clone(),
            current_head: context.source_sha.clone(),
            canonical_url: format!(
                "https://github.com/contributor/project/tree/{}",
                context.branch_name()
            ),
            created_at: NOW.into(),
        }))
    }

    async fn reconcile_branch(
        &self,
        _context: &BranchContext,
    ) -> Result<BranchReconciliation, GithubBrokerError> {
        self.branch_reconcile_calls.fetch_add(1, Ordering::SeqCst);
        Err(GithubBrokerError::Malformed(
            "the release happy path must not reconcile its branch".into(),
        ))
    }

    async fn import_workspace(
        &self,
        context: &WorkspaceContext,
    ) -> Result<WorkspaceSourcePlan, GithubBrokerError> {
        self.workspace_import_calls.fetch_add(1, Ordering::SeqCst);
        Ok(WorkspaceSourcePlan {
            source_revision: context.branch.source_sha.clone(),
            tree_digest: "f".repeat(64),
            entries: Vec::new(),
            blobs: Vec::new(),
        })
    }

    async fn read_workspace_blob(
        &self,
        _context: &WorkspaceContext,
        _blob: &BaselineImportBlob,
    ) -> Result<Vec<u8>, GithubBrokerError> {
        Err(GithubBrokerError::Malformed(
            "the release fixture has no provider blobs".into(),
        ))
    }

    async fn preflight_capability(
        &self,
        launch: &Launch,
    ) -> Result<CapabilityAuthorityEvidence, GithubBrokerError> {
        self.capability_preflight_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(CapabilityAuthorityEvidence {
            connection: launch.connection.clone().unwrap(),
            authority: launch.authority.clone().unwrap(),
            upstream: launch.revision.content.upstream.repository.clone(),
            fork: launch.fork.as_ref().unwrap().repository.clone(),
            branch: launch.branch.clone().unwrap(),
        })
    }

    async fn preflight_effect_authority(
        &self,
        launch: &Launch,
        kind: LaunchEffectKind,
    ) -> Result<EffectAuthorityEvidence, GithubBrokerError> {
        self.effect_authority_preflights.lock().unwrap().push(kind);
        if matches!(
            kind,
            LaunchEffectKind::Reach
                | LaunchEffectKind::BranchPublication
                | LaunchEffectKind::PullRequest
        ) {
            let evidence = self.preflight_capability(launch).await?;
            return Ok(EffectAuthorityEvidence {
                connection: evidence.connection,
                authority: evidence.authority,
                upstream: evidence.upstream,
                fork: Some(evidence.fork),
                branch: Some(evidence.branch),
            });
        }
        Ok(EffectAuthorityEvidence {
            connection: launch.connection.clone().unwrap(),
            authority: launch.authority.clone().unwrap(),
            upstream: launch.revision.content.upstream.repository.clone(),
            fork: if kind == LaunchEffectKind::Branch {
                launch.fork.as_ref().map(|fork| fork.repository.clone())
            } else {
                None
            },
            branch: None,
        })
    }

    async fn publish_branch(
        &self,
        _context: &crate::trigger::github::PublicationContext,
        _plan: &crate::trigger::workspace_diff::PublicationPlan,
    ) -> Result<PublicationAdvance, GithubBrokerError> {
        self.publication_calls.fetch_add(1, Ordering::SeqCst);
        let answer = self
            .publication_answers
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| GithubBrokerError::Malformed("duplicate publication call".into()))?;
        match answer {
            PublicationAdvance::ObjectsWritten(_) => {
                self.publication_object_writes
                    .fetch_add(1, Ordering::SeqCst);
            }
            PublicationAdvance::RefUpdatePending(_) => {
                self.publication_ref_checks.fetch_add(1, Ordering::SeqCst);
            }
            PublicationAdvance::Published(_) => {
                self.publication_ref_writes.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(answer)
    }

    async fn write_pull_request(
        &self,
        _context: &crate::trigger::github::PullRequestContext,
    ) -> Result<PullRequestAdvance, GithubBrokerError> {
        self.pull_request_write_calls.fetch_add(1, Ordering::SeqCst);
        self.pull_request_answer
            .lock()
            .unwrap()
            .take()
            .map(|answer| PullRequestAdvance::Ready(Box::new(answer)))
            .ok_or_else(|| GithubBrokerError::Malformed("duplicate pull-request write".into()))
    }

    async fn reconcile_pull_request(
        &self,
        _context: &crate::trigger::github::PullRequestContext,
    ) -> Result<PullRequestAdvance, GithubBrokerError> {
        self.pull_request_reconcile_calls
            .fetch_add(1, Ordering::SeqCst);
        Err(GithubBrokerError::Malformed(
            "the release happy path must not reconcile its pull request".into(),
        ))
    }
}

#[derive(Default)]
struct ReleaseSessions {
    ensure_calls: AtomicUsize,
    runtime_identity_calls: AtomicUsize,
    policy_install_calls: AtomicUsize,
    gateway_probe_calls: AtomicUsize,
    prompt_calls: AtomicUsize,
    checkpoint_calls: AtomicUsize,
    observed_session_ids: Mutex<Vec<SessionId>>,
    runtime_policy: Mutex<Option<layer2_core::RuntimePolicyObservation>>,
    credential_marker: &'static str,
}

#[async_trait]
impl TriggerSessionEffectPort for ReleaseSessions {
    async fn ensure_session(&self, launch: &Launch) -> Result<TriggerSessionEvidence, CoreError> {
        self.ensure_calls.fetch_add(1, Ordering::SeqCst);
        let session = trigger_session(launch, SessionStatus::Ready, 0);
        self.observed_session_ids
            .lock()
            .unwrap()
            .push(session.id.clone());
        Ok(TriggerSessionEvidence {
            runtime_generation: Some(1),
            credential_authority: Some(layer2_core::SessionCredentialAuthority {
                session_id: session.id.clone(),
                custody_generation: 1,
                provider_key_id: "provider-session-key-1".into(),
                provider_state_version: 1,
                provider_expires_at_epoch_milliseconds: 4_000_000_000_000,
                grant_fingerprint: "a".repeat(64),
            }),
            session,
        })
    }

    async fn credential_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<layer2_core::SessionCredentialAuthority>, CoreError> {
        Ok(Some(layer2_core::SessionCredentialAuthority {
            session_id: session_id.clone(),
            custody_generation: 1,
            provider_key_id: "provider-session-key-1".into(),
            provider_state_version: 1,
            provider_expires_at_epoch_milliseconds: 4_000_000_000_000,
            grant_fingerprint: "a".repeat(64),
        }))
    }

    async fn install_runtime_policy_and_probe(
        &self,
        _session_id: &SessionId,
        _expected_credential_authority: &layer2_core::SessionCredentialAuthority,
        mut capability: LaunchCapability,
        signer: Arc<CapabilitySigner>,
        probe: layer3_adapter::RuntimeGatewayProbeRequest,
    ) -> Result<
        layer2_core::RuntimeGatewayReachabilityReceipt,
        layer2_core::RuntimeGatewayReachabilityError,
    > {
        self.policy_install_calls.fetch_add(1, Ordering::SeqCst);
        self.gateway_probe_calls.fetch_add(1, Ordering::SeqCst);
        capability.runtime_audience = "release-runtime-1".into();
        signer
            .sign_policy(&capability)
            .map_err(CoreError::InvalidRequest)
            .map_err(layer2_core::RuntimeGatewayReachabilityError::Core)?;
        *self.runtime_policy.lock().unwrap() = Some(layer2_core::RuntimePolicyObservation {
            runtime_audience: capability.runtime_audience.clone(),
            required: true,
            supported: true,
            installed: true,
            revoked: false,
            launch_id: Some(capability.launch_id.0.clone()),
            generation: Some(capability.generation),
            fence: Some(capability.fence),
            expires_at_epoch_seconds: Some(capability.expires_at_epoch_seconds),
            last_refusal: None,
        });
        Ok(layer2_core::RuntimeGatewayReachabilityReceipt {
            runtime_audience: capability.runtime_audience,
            policy: layer3_adapter::RuntimePolicyReceipt {
                generation: capability.generation,
                fence: capability.fence,
                revoked: false,
            },
            probe_version: probe.version,
        })
    }

    async fn runtime_policy_audience(&self, _session_id: &SessionId) -> Result<String, CoreError> {
        self.runtime_identity_calls.fetch_add(1, Ordering::SeqCst);
        Ok("release-runtime-1".into())
    }

    async fn runtime_policy_observation(
        &self,
        _session_id: &SessionId,
    ) -> Result<layer2_core::RuntimePolicyObservation, CoreError> {
        self.runtime_identity_calls.fetch_add(1, Ordering::SeqCst);
        self.runtime_policy
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| CoreError::RuntimeRefused("release policy was not installed".into()))
    }

    async fn install_runtime_tombstone(
        &self,
        _session_id: &SessionId,
        _tombstone: RuntimePolicyTombstone,
        _signer: Arc<CapabilitySigner>,
    ) -> Result<(String, layer3_adapter::RuntimePolicyReceipt), CoreError> {
        Err(CoreError::CapabilityUnsupported(
            "the happy path does not revoke its policy".into(),
        ))
    }

    async fn retire_runtime_credential(&self, _session_id: &SessionId) -> Result<(), CoreError> {
        Err(CoreError::CapabilityUnsupported(
            "the happy path does not retire its credential".into(),
        ))
    }

    async fn runtime_credential_retirement_settled(
        &self,
        _session_id: &SessionId,
    ) -> Result<bool, CoreError> {
        Ok(false)
    }

    async fn dispatch_initial_prompt(
        &self,
        launch: &Launch,
        _claim: &EffectClaim,
    ) -> Result<PromptDelivery, CoreError> {
        self.prompt_calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(launch.owner, SubjectId("contributor".into()));
        assert!(launch.effective.rendered_prompt.contains(PROMPT_MARKER));
        assert!(!launch
            .effective
            .rendered_prompt
            .contains(self.credential_marker));
        Ok(prompt_delivery(launch, PromptDeliveryState::Completed))
    }

    async fn checkpoint_workspace(
        &self,
        launch: &Launch,
        request: &layer3_adapter::WorkspaceCheckpointRequest,
    ) -> Result<(layer3_adapter::WorkspaceCheckpointPackage, u64), CoreError> {
        self.checkpoint_calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.epoch, 1);
        assert_eq!(request.version, 1);
        assert_eq!(request.source_revision, launch.source_sha);
        assert_eq!(
            request.baseline_manifest_digest,
            launch.workspace.as_ref().unwrap().baseline.digest
        );
        Ok((checkpoint_package(launch, request), 1))
    }
}

#[derive(Default)]
struct ReleaseObjects {
    baseline: Mutex<Option<Vec<u8>>>,
    completed_baseline: Mutex<Option<BaselineImportOutcome>>,
    checkpoint: Mutex<Option<Vec<u8>>>,
    diff: Mutex<Option<Vec<u8>>>,
    baseline_puts: AtomicUsize,
    baseline_gets: AtomicUsize,
    checkpoint_puts: AtomicUsize,
    checkpoint_gets: AtomicUsize,
    diff_puts: AtomicUsize,
    diff_gets: AtomicUsize,
}

#[async_trait]
impl BaselineObjectStore for ReleaseObjects {
    async fn reconcile_baseline_import(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        source_revision: &str,
    ) -> Result<Option<BaselineImportOutcome>, TriggerStoreError> {
        let completed = self.completed_baseline.lock().unwrap().clone();
        if completed
            .as_ref()
            .is_some_and(|outcome| outcome.manifest.source_revision != source_revision)
        {
            return Err(TriggerStoreError::Malformed(
                "release baseline source changed".into(),
            ));
        }
        Ok(completed)
    }

    async fn begin_baseline_import(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        _source_revision: &str,
        _tree_digest: &str,
        blobs: &[BaselineImportBlob],
    ) -> Result<(), TriggerStoreError> {
        assert!(blobs.is_empty(), "the release baseline fixture is empty");
        Ok(())
    }

    async fn get_baseline_import_chunk(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        _source_revision: &str,
        _blob: &BaselineImportBlob,
        _ordinal: u32,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        Err(TriggerStoreError::Malformed(
            "the release baseline fixture has no chunks".into(),
        ))
    }

    async fn put_baseline_import_chunk(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        _source_revision: &str,
        _blob: &BaselineImportBlob,
        _chunk: &BaselineImportChunk,
        _bytes: &[u8],
    ) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::Malformed(
            "the release baseline fixture has no chunks".into(),
        ))
    }

    async fn finalize_baseline_import(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        manifest: &WorkspaceManifest,
        plan: BaselineArchivePlan,
    ) -> Result<BaselineImportOutcome, TriggerStoreError> {
        assert!(
            plan.chunks.is_empty(),
            "the release baseline fixture is empty"
        );
        assert_eq!(plan.archive_bytes, plan.preamble.len() as u64);
        self.baseline_puts.fetch_add(1, Ordering::SeqCst);
        let mut stored = self.baseline.lock().unwrap();
        assert!(stored.is_none(), "the baseline is written exactly once");
        *stored = Some(plan.preamble.clone());
        let outcome = BaselineImportOutcome {
            manifest: manifest.clone(),
            object_reference: "release/baseline.pack".into(),
            archive_sha256: format!("{:x}", Sha256::digest(&plan.preamble)),
            archive_bytes: plan.archive_bytes,
        };
        *self.completed_baseline.lock().unwrap() = Some(outcome.clone());
        Ok(outcome)
    }

    async fn retire_baseline_import_chunks(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
    ) -> Result<(), TriggerStoreError> {
        Ok(())
    }

    async fn delete_baseline_import(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
    ) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::Malformed(
            "the happy path does not delete its baseline import".into(),
        ))
    }

    async fn put_baseline(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        _manifest: &WorkspaceManifest,
        _archive_sha256: &str,
        archive: &[u8],
    ) -> Result<String, TriggerStoreError> {
        self.baseline_puts.fetch_add(1, Ordering::SeqCst);
        let mut stored = self.baseline.lock().unwrap();
        assert!(stored.is_none(), "the baseline is written exactly once");
        *stored = Some(archive.to_vec());
        Ok("release/baseline.pack".into())
    }

    async fn get_baseline(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        reference: &str,
        _archive_sha256: &str,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        self.baseline_gets.fetch_add(1, Ordering::SeqCst);
        assert_eq!(reference, "release/baseline.pack");
        Ok(self.baseline.lock().unwrap().clone())
    }

    async fn delete_baseline(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        _reference: &str,
    ) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::Malformed(
            "the happy path does not delete its baseline".into(),
        ))
    }
}

#[async_trait]
impl CheckpointObjectStore for ReleaseObjects {
    async fn put_checkpoint(
        &self,
        _owner: &SubjectId,
        _checkpoint: &WorkspaceCheckpoint,
        archive: &[u8],
    ) -> Result<String, TriggerStoreError> {
        self.checkpoint_puts.fetch_add(1, Ordering::SeqCst);
        let mut stored = self.checkpoint.lock().unwrap();
        assert!(stored.is_none(), "the checkpoint is written exactly once");
        *stored = Some(archive.to_vec());
        Ok("release/checkpoint-v1.pack".into())
    }

    async fn get_checkpoint(
        &self,
        _owner: &SubjectId,
        _checkpoint: &WorkspaceCheckpoint,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        self.checkpoint_gets.fetch_add(1, Ordering::SeqCst);
        Ok(self.checkpoint.lock().unwrap().clone())
    }

    async fn delete_checkpoint(
        &self,
        _owner: &SubjectId,
        _checkpoint: &WorkspaceCheckpoint,
    ) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::Malformed(
            "the happy path does not delete its checkpoint".into(),
        ))
    }

    async fn delete_checkpoint_intent(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        _intent: &layer2_core::WorkspaceCheckpointIntent,
    ) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::Malformed(
            "the happy path does not delete its checkpoint intent".into(),
        ))
    }
}

#[async_trait]
impl DiffArtifactObjectStore for ReleaseObjects {
    async fn put_diff_artifact(
        &self,
        _owner: &SubjectId,
        _launch: &layer2_core::LaunchId,
        _artifact: &layer2_core::DiffArtifactId,
        _sha256: &str,
        bytes: &[u8],
    ) -> Result<String, TriggerStoreError> {
        self.diff_puts.fetch_add(1, Ordering::SeqCst);
        let mut stored = self.diff.lock().unwrap();
        assert!(
            stored.is_none(),
            "the diff artifact is written exactly once"
        );
        *stored = Some(bytes.to_vec());
        Ok("release/diff-v1.json".into())
    }

    async fn get_diff_artifact(
        &self,
        _owner: &SubjectId,
        _artifact: &DiffArtifact,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        self.diff_gets.fetch_add(1, Ordering::SeqCst);
        Ok(self.diff.lock().unwrap().clone())
    }

    async fn delete_diff_artifact(
        &self,
        _owner: &SubjectId,
        _artifact: &DiffArtifact,
    ) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::Malformed(
            "the happy path does not delete its diff".into(),
        ))
    }
}

#[test]
fn one_launch_reaches_one_reviewed_commit_and_one_pull_request() {
    std::thread::Builder::new()
        .name("cma-trigger-release-journey".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("the release verification runtime builds")
                .block_on(run_release_journey());
        })
        .expect("the release verification thread starts")
        .join()
        .expect("the release verification journey completes");
}

async fn run_release_journey() {
    let initial = launch_with_prompt_marker();
    let owner = initial.owner.clone();
    let launch_id = initial.id.clone();
    let service = service_for_launch(initial);
    let github = Arc::new(ReleaseGithub::new());
    let sessions = Arc::new(ReleaseSessions {
        credential_marker: CREDENTIAL_MARKER,
        ..ReleaseSessions::default()
    });
    let objects = Arc::new(ReleaseObjects::default());
    let activity = Arc::new(RecordingActivityStore::default());
    let executor = Arc::new(GithubEffectExecutor {
        github: github.clone(),
        workspace_store: Some(objects.clone()),
        checkpoint_store: Some(objects.clone()),
        diff_store: Some(objects.clone()),
        sessions: Some(sessions.clone()),
        capability_signer: Some(test_signer()),
        fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
        changed_file_limit: TEST_CHANGED_FILE_LIMIT,
    });
    let worker = EffectWorker::new(service.clone(), executor, "release-verification")
        .unwrap()
        .with_activity(ActivityService::new(activity.clone()));

    for effect in [
        LaunchEffectKind::Preflight,
        LaunchEffectKind::Fork,
        LaunchEffectKind::Branch,
        LaunchEffectKind::WorkspaceImport,
        LaunchEffectKind::Sandbox,
        LaunchEffectKind::Session,
        LaunchEffectKind::Reach,
        LaunchEffectKind::InitialDispatch,
    ] {
        run_effect(
            &service,
            &worker,
            &owner,
            &launch_id,
            effect,
            EffectRunOutcome::Completed,
        )
        .await;
    }

    let ready = service.get_launch(&owner, &launch_id).await.unwrap();
    assert_eq!(ready.state, LaunchState::Ready);
    assert_eq!(ready.fork.as_ref().unwrap().repository.repository_id, 30);
    assert_eq!(
        ready.branch.as_ref().unwrap().name,
        format!("cma/{}", launch_id.0)
    );
    assert_eq!(
        ready.session.as_ref().unwrap().session_id.0,
        "sesn_trigger_effect"
    );
    assert_eq!(
        ready.delivery.as_ref().unwrap().state,
        PromptDeliveryState::Completed
    );
    assert_eq!(
        service
            .list_launches(
                &owner,
                None,
                Vec::new(),
                layer2_core::TriggerPageRequest::default(),
            )
            .await
            .unwrap()
            .items
            .len(),
        1
    );

    let reviewing = service
        .request_diff_review(
            &owner,
            &launch_id,
            ready.revision_number,
            confirmation(
                ConfirmationAction::ReviewDiff,
                &ready,
                ready.effective.digest.clone(),
                "review-diff",
            ),
            NOW,
            "review-diff",
        )
        .await
        .unwrap();
    assert_eq!(reviewing.state, LaunchState::Contributing);
    run_effect(
        &service,
        &worker,
        &owner,
        &launch_id,
        LaunchEffectKind::Checkpoint,
        EffectRunOutcome::Completed,
    )
    .await;
    run_effect(
        &service,
        &worker,
        &owner,
        &launch_id,
        LaunchEffectKind::Diff,
        EffectRunOutcome::Completed,
    )
    .await;

    let reviewed = service.get_launch(&owner, &launch_id).await.unwrap();
    let diff = reviewed.diff.as_ref().unwrap();
    assert_eq!(diff.validation, DiffValidationOutcome::Valid);
    assert_eq!(diff.total_changed_files, 1);
    assert_eq!(diff.files.len(), 1);
    assert_eq!(diff.files[0].path, PATH_MARKER);
    assert_eq!(diff.files[0].change, DiffChangeKind::Added);
    assert_eq!(diff.author.name, "contributor");
    assert_eq!(diff.author.email, "21+contributor@users.noreply.github.com");
    assert_eq!(diff.committer.name, "CMA");
    assert_eq!(diff.committer.email, "cma@chrono.ai");

    let publication = service
        .confirm_branch_publication(
            &owner,
            &launch_id,
            reviewed.revision_number,
            confirmation(
                ConfirmationAction::PublishBranch,
                &reviewed,
                diff.digest.clone(),
                "publish-branch",
            ),
            NOW,
            "publish-branch",
        )
        .await
        .unwrap();
    assert_eq!(publication.state, LaunchState::PrPending);
    for outcome in [
        EffectRunOutcome::Deferred,
        EffectRunOutcome::Deferred,
        EffectRunOutcome::Completed,
    ] {
        run_effect(
            &service,
            &worker,
            &owner,
            &launch_id,
            LaunchEffectKind::BranchPublication,
            outcome,
        )
        .await;
    }

    let published = service.get_launch(&owner, &launch_id).await.unwrap();
    let contribution = published.contribution.as_ref().unwrap();
    assert_eq!(contribution.state, PublicationState::Published);
    assert_eq!(
        contribution.commit_id.as_deref(),
        Some("dddddddddddddddddddddddddddddddddddddddd")
    );
    assert_eq!(contribution.resulting_head, contribution.commit_id);
    assert_eq!(contribution.author, diff.author);
    assert_eq!(contribution.committer, diff.committer);
    let draft = published
        .pull_request_draft(PullRequestOperation::Open, None, None)
        .unwrap();
    let pr_pending = service
        .confirm_pull_request(
            &owner,
            &launch_id,
            layer2_core::ConfirmPullRequestCommand {
                expected_revision: published.revision_number,
                confirmation: confirmation(
                    ConfirmationAction::OpenPullRequest,
                    &published,
                    draft.intent_digest,
                    "open-pr",
                ),
                title: None,
                body: None,
                now: NOW.into(),
                correlation_id: "open-pr".into(),
            },
        )
        .await
        .unwrap();
    github.set_pull_request_answer(reconciled_pull_request(&pr_pending, PullRequestState::Open));
    run_effect(
        &service,
        &worker,
        &owner,
        &launch_id,
        LaunchEffectKind::PullRequest,
        EffectRunOutcome::Completed,
    )
    .await;

    let complete = service.get_launch(&owner, &launch_id).await.unwrap();
    assert_eq!(complete.state, LaunchState::PrOpen);
    assert_eq!(complete.pull_request.as_ref().unwrap().number, Some(7));
    assert_eq!(
        complete.pull_request.as_ref().unwrap().state,
        PullRequestState::Open
    );
    let before_duplicate = release_counts(&github, &sessions, &objects);
    assert_eq!(
        worker
            .run_once(
                &launch_id,
                complete.revision_number,
                "duplicate-worker-click"
            )
            .await
            .unwrap(),
        EffectRunOutcome::NoWork
    );
    assert_eq!(
        release_counts(&github, &sessions, &objects),
        before_duplicate
    );

    assert_eq!(github.preflight_calls.load(Ordering::SeqCst), 1);
    assert_eq!(github.fork_create_calls.load(Ordering::SeqCst), 1);
    assert_eq!(github.fork_reconcile_calls.load(Ordering::SeqCst), 0);
    assert_eq!(github.branch_create_calls.load(Ordering::SeqCst), 1);
    assert_eq!(github.branch_reconcile_calls.load(Ordering::SeqCst), 0);
    assert_eq!(github.workspace_import_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *github.effect_authority_preflights.lock().unwrap(),
        vec![
            LaunchEffectKind::Fork,
            LaunchEffectKind::Branch,
            LaunchEffectKind::Reach,
            LaunchEffectKind::BranchPublication,
            LaunchEffectKind::BranchPublication,
            LaunchEffectKind::BranchPublication,
            LaunchEffectKind::PullRequest,
        ]
    );
    // Five write admissions plus the read-only diff authority observation.
    assert_eq!(github.capability_preflight_calls.load(Ordering::SeqCst), 6);
    assert_eq!(github.publication_calls.load(Ordering::SeqCst), 3);
    assert_eq!(github.publication_object_writes.load(Ordering::SeqCst), 1);
    assert_eq!(github.publication_ref_checks.load(Ordering::SeqCst), 1);
    assert_eq!(github.publication_ref_writes.load(Ordering::SeqCst), 1);
    assert_eq!(github.pull_request_write_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        github.pull_request_reconcile_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(sessions.ensure_calls.load(Ordering::SeqCst), 2);
    assert_eq!(sessions.runtime_identity_calls.load(Ordering::SeqCst), 1);
    assert_eq!(sessions.policy_install_calls.load(Ordering::SeqCst), 1);
    assert_eq!(sessions.prompt_calls.load(Ordering::SeqCst), 1);
    assert_eq!(sessions.checkpoint_calls.load(Ordering::SeqCst), 1);
    assert_eq!(objects.baseline_puts.load(Ordering::SeqCst), 1);
    assert_eq!(objects.checkpoint_puts.load(Ordering::SeqCst), 1);
    assert_eq!(objects.diff_puts.load(Ordering::SeqCst), 1);

    {
        let observed_sessions = sessions.observed_session_ids.lock().unwrap();
        assert_eq!(observed_sessions.len(), 2);
        assert!(
            observed_sessions
                .iter()
                .all(|session| session == &complete.session.as_ref().unwrap().session_id),
            "sandbox readiness and attachment must resolve one deterministic session"
        );
    }

    let support_caller = Caller::new(
        SubjectId("release-support".into()),
        Authorization::from_capabilities([Capability::ReadActivityLog]),
    );
    let support = service
        .support(
            &support_caller,
            TriggerSupportLookup::Launch(launch_id.clone()),
        )
        .await
        .unwrap();
    let owner_projection = serde_json::to_string(&complete).unwrap();
    let support_projection = serde_json::to_string(&support).unwrap();
    let activity_records = activity.records.lock().await;
    assert_eq!(activity_records.len(), 14);
    let audit_projection = serde_json::to_string(&*activity_records).unwrap();
    drop(activity_records);
    let checkpoint_projection = objects.checkpoint.lock().unwrap().clone().unwrap();
    let diff_projection = objects.diff.lock().unwrap().clone().unwrap();

    assert_eq!(github.provider_body_marker, PROVIDER_BODY_MARKER);
    assert_eq!(sessions.credential_marker, CREDENTIAL_MARKER);
    assert!(owner_projection.contains(PROMPT_MARKER));
    assert!(owner_projection.contains(PATH_MARKER));
    assert!(String::from_utf8_lossy(&checkpoint_projection).contains(PATH_MARKER));
    assert!(String::from_utf8_lossy(&diff_projection).contains(PATH_MARKER));
    assert!(String::from_utf8_lossy(&diff_projection)
        .contains("WORKSPACE_CONTENT_SENTINEL_RELEASE_673"));
    assert_projection_excludes(
        "owner Launch",
        owner_projection.as_bytes(),
        &[CREDENTIAL_MARKER, PROVIDER_BODY_MARKER],
    );
    assert_projection_excludes(
        "redacted support",
        support_projection.as_bytes(),
        &[
            CREDENTIAL_MARKER,
            PROVIDER_BODY_MARKER,
            PROMPT_MARKER,
            PATH_MARKER,
        ],
    );
    assert_projection_excludes(
        "worker audit",
        audit_projection.as_bytes(),
        &[
            CREDENTIAL_MARKER,
            PROVIDER_BODY_MARKER,
            PROMPT_MARKER,
            PATH_MARKER,
        ],
    );
    for (name, projection) in [
        (
            "baseline artifact",
            objects.baseline.lock().unwrap().clone().unwrap(),
        ),
        ("checkpoint artifact", checkpoint_projection),
        ("diff artifact", diff_projection),
    ] {
        assert_projection_excludes(
            name,
            &projection,
            &[CREDENTIAL_MARKER, PROVIDER_BODY_MARKER, PROMPT_MARKER],
        );
    }

    for step in &complete.steps {
        for key in step.external_resource_ids.keys() {
            assert!(
                !matches!(
                    key.as_str(),
                    "access_token" | "authorization" | "credential" | "github_token" | "secret"
                ),
                "unsafe external resource key: {key}"
            );
        }
    }
}

async fn run_effect(
    service: &Arc<TriggerService>,
    worker: &EffectWorker,
    owner: &SubjectId,
    launch_id: &layer2_core::LaunchId,
    expected_effect: LaunchEffectKind,
    expected_outcome: EffectRunOutcome,
) {
    let before = service.get_launch(owner, launch_id).await.unwrap();
    assert_eq!(before.next_effect(), Some(expected_effect));
    let observed = worker
        .run_once(
            launch_id,
            before.revision_number,
            &format!("release-{expected_effect:?}"),
        )
        .await
        .unwrap();
    assert_eq!(
        observed, expected_outcome,
        "unexpected worker outcome for {expected_effect:?} from {:?}",
        before.state
    );
}

fn confirmation(
    action: ConfirmationAction,
    launch: &Launch,
    snapshot_digest: String,
    key: &str,
) -> Confirmation {
    Confirmation {
        action,
        subject: launch.owner.clone(),
        snapshot_digest,
        confirmed_at: NOW.into(),
        idempotency_key_hash: format!("{key}-hash"),
    }
}

fn launch_with_prompt_marker() -> Launch {
    let mut launch = launch_fixture();
    launch.revision.content.task.prompt_template = format!("{PROMPT_MARKER}: Fix {{{{topic}}}}");
    launch.revision.digest = launch.revision.content.digest().unwrap();
    launch.revision_digest = launch.revision.digest.clone();
    launch.effective = layer2_core::effective_configuration(&launch.revision, &launch.overrides)
        .expect("the planted prompt remains a valid Trigger configuration");
    for confirmation in &mut launch.confirmations {
        if confirmation.action == ConfirmationAction::Start {
            confirmation.snapshot_digest = launch.effective.digest.clone();
        }
    }
    launch
}

fn checkpoint_package(
    launch: &Launch,
    request: &layer3_adapter::WorkspaceCheckpointRequest,
) -> layer3_adapter::WorkspaceCheckpointPackage {
    let content_sha256 = format!("{:x}", Sha256::digest(CHECKPOINT_CONTENT));
    let entry = WorkspaceEntry {
        path: PATH_MARKER.into(),
        kind: WorkspaceEntryKind::Regular,
        mode: 0o100644,
        size: CHECKPOINT_CONTENT.len() as u64,
        content_sha256: content_sha256.clone(),
        provider_object_id: Some(crate::trigger::workspace_diff::git_blob_id(
            CHECKPOINT_CONTENT,
        )),
        symlink_target: None,
    };
    let manifest = WorkspaceManifest::seal(launch.source_sha.clone(), vec![entry.clone()]).unwrap();
    let archive = test_archive(
        &manifest,
        &[(entry, CHECKPOINT_CONTENT.to_vec())],
        request.version,
        1,
    );
    layer3_adapter::WorkspaceCheckpointPackage {
        epoch: request.epoch,
        version: request.version,
        source_revision: request.source_revision.clone(),
        manifest_digest: manifest.digest,
        archive_sha256: format!("{:x}", Sha256::digest(&archive)),
        entries: vec![layer3_adapter::WorkspaceEntryPackage {
            path: PATH_MARKER.into(),
            kind: layer3_adapter::WorkspaceEntryKind::Regular,
            mode: 0o100644,
            size: CHECKPOINT_CONTENT.len() as u64,
            content_sha256,
            provider_object_id: Some(crate::trigger::workspace_diff::git_blob_id(
                CHECKPOINT_CONTENT,
            )),
            symlink_target: None,
        }],
        total_bytes: CHECKPOINT_CONTENT.len() as u64,
        quiesced_writers: 1,
        writer_state: layer3_adapter::WorkspaceWriterState::Resumed,
        writer_resume_token: None,
        archive,
    }
}

fn release_counts(
    github: &ReleaseGithub,
    sessions: &ReleaseSessions,
    objects: &ReleaseObjects,
) -> Vec<usize> {
    vec![
        github.preflight_calls.load(Ordering::SeqCst),
        github.fork_create_calls.load(Ordering::SeqCst),
        github.branch_create_calls.load(Ordering::SeqCst),
        github.workspace_import_calls.load(Ordering::SeqCst),
        github.capability_preflight_calls.load(Ordering::SeqCst),
        github.publication_calls.load(Ordering::SeqCst),
        github.publication_object_writes.load(Ordering::SeqCst),
        github.publication_ref_checks.load(Ordering::SeqCst),
        github.publication_ref_writes.load(Ordering::SeqCst),
        github.pull_request_write_calls.load(Ordering::SeqCst),
        sessions.ensure_calls.load(Ordering::SeqCst),
        sessions.runtime_identity_calls.load(Ordering::SeqCst),
        sessions.policy_install_calls.load(Ordering::SeqCst),
        sessions.prompt_calls.load(Ordering::SeqCst),
        sessions.checkpoint_calls.load(Ordering::SeqCst),
        objects.baseline_puts.load(Ordering::SeqCst),
        objects.checkpoint_puts.load(Ordering::SeqCst),
        objects.diff_puts.load(Ordering::SeqCst),
    ]
}

fn assert_projection_excludes(name: &str, projection: &[u8], markers: &[&str]) {
    for marker in markers {
        assert!(
            !projection
                .windows(marker.len())
                .any(|window| window == marker.as_bytes()),
            "{name} leaked {marker}"
        );
    }
}
