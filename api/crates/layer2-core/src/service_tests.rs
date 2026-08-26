//! Tests for the session service's terminal relay.
//!
//! Kept in a sibling file so `service.rs` stays within the project's file-size
//! limit while the behaviour remains covered.

use super::*;
use crate::terminal::{ScreenSnapshot, TerminalGeometry};
use crate::terminal_io::spawn_relay;
use crate::test_support::{service, spec, user, FakeAdapter, FakeStore};
use crate::SubjectId;
use async_trait::async_trait;
use layer3_adapter::TerminalOutput;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A screen model that records what it was fed, so the relay's behaviour
/// can be observed without a real terminal parser.
struct RecordingScreen {
    geometry: TerminalGeometry,
    seen: Vec<u8>,
}

impl ScreenModel for RecordingScreen {
    fn apply(&mut self, bytes: &[u8]) {
        self.seen.extend_from_slice(bytes);
    }

    fn resize(&mut self, geometry: TerminalGeometry) {
        self.geometry = geometry;
    }

    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.geometry,
            rows: vec![String::from_utf8_lossy(&self.seen).into_owned()],
        }
    }

    fn repaint_bytes(&self) -> Vec<u8> {
        self.seen.clone()
    }
}

#[tokio::test]
async fn the_relay_feeds_the_screen_with_no_client_attached() {
    // The whole point of the relay: a detached session keeps working, and
    // its screen stays current so a later attach is correct.
    let service = service(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
    );
    let (tx, rx) = mpsc::channel::<TerminalOutput>(8);
    let geometry = TerminalGeometry::default();
    let screen: Arc<Mutex<Box<dyn ScreenModel>>> =
        Arc::new(Mutex::new(Box::new(RecordingScreen {
            geometry,
            seen: Vec::new(),
        })));
    let (live, _) = broadcast::channel(8);
    let (ending, _) = tokio::sync::watch::channel(None);

    spawn_relay(
        service,
        SessionId("sesn_test".into()),
        rx,
        Arc::clone(&screen),
        live,
        ending,
    );

    tx.send(TerminalOutput::Bytes(b"painted".to_vec()))
        .await
        .expect("relay accepts output");
    // Yield until the relay has drained the channel.
    for _ in 0..50 {
        if !screen.lock().await.repaint_bytes().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        screen.lock().await.repaint_bytes(),
        b"painted".to_vec(),
        "the screen must absorb output even with nobody attached"
    );
}

#[tokio::test]
async fn output_with_no_subscriber_is_not_an_error() {
    // A send into a broadcast with no receivers fails; the relay must treat
    // that as "detached", not as a fault that ends the session.
    let service = service(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
    );
    let (tx, rx) = mpsc::channel::<TerminalOutput>(8);
    let screen: Arc<Mutex<Box<dyn ScreenModel>>> =
        Arc::new(Mutex::new(Box::new(RecordingScreen {
            geometry: TerminalGeometry::default(),
            seen: Vec::new(),
        })));
    let (live, _) = broadcast::channel(8);
    let (ending, _) = tokio::sync::watch::channel(None);
    spawn_relay(
        service,
        SessionId("sesn_test".into()),
        rx,
        Arc::clone(&screen),
        live,
        ending,
    );

    for chunk in [b"one".to_vec(), b"two".to_vec()] {
        tx.send(TerminalOutput::Bytes(chunk))
            .await
            .expect("relay still accepting");
    }
    for _ in 0..50 {
        if screen.lock().await.repaint_bytes().len() >= 6 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(screen.lock().await.repaint_bytes(), b"onetwo".to_vec());
}

#[tokio::test]
async fn a_session_runs_on_the_kernel_this_deployment_configured() {
    // A caller cannot name one, and the type is what says so: the field a
    // caller would have filled is gone, so an interface cannot reach past the
    // decision by substituting a literal — which is what both of them did, and
    // what the core then silently overwrote.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            ..Default::default()
        });
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        profiles,
    );
    let alice = user("alice");

    let created = svc.create_session(&alice, spec()).await.expect("created");
    assert_eq!(
        created.kernel,
        crate::test_support::kernel(),
        "the deployment's default is what an unspecified session gets"
    );
}

#[tokio::test]
async fn a_kernel_this_deployment_does_not_run_is_refused_before_anything_is_recorded() {
    // The profile is where a kernel is named, so a profile naming one this
    // deployment cannot start must fail the request rather than leave a session
    // provisioning towards a runtime that does not exist.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::ids::KernelId::new("a-kernel-this-deployment-does-not-run"),
            ..Default::default()
        });
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        profiles,
    );
    let alice = user("alice");

    let err = svc
        .create_session(&alice, spec())
        .await
        .expect_err("a kernel this deployment cannot start must not be accepted");
    assert!(
        matches!(err, CoreError::RuntimeUnavailable(_)),
        "got {err:?}"
    );
    assert!(
        svc.list_sessions(&alice, &crate::SessionListQuery::default())
            .await
            .expect("session list is readable")
            .items
            .is_empty(),
        "nothing may be recorded for a session that was refused"
    );
}

#[tokio::test]
async fn a_runtime_is_advertised_as_what_it_says_it_can_be() {
    // What a client is offered comes from the adapter, not from a list kept
    // beside it: a kernel that gains the conversation plane is offered for it
    // the moment it says so, and one that loses a plane stops being offered
    // without anything here being edited.
    let store = Arc::new(FakeStore::default());
    let screen_only = Arc::new(FakeAdapter::default());
    let svc = crate::test_support::service(Arc::clone(&store), screen_only);

    assert_eq!(
        svc.available_kernels(),
        vec![(crate::test_support::kernel(), vec![SessionMode::Terminal])],
        "the fake runtime renders a screen and holds no conversation"
    );

    let both = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    });
    let svc = crate::test_support::service(Arc::clone(&store), both);

    assert_eq!(
        svc.available_kernels(),
        vec![(
            crate::test_support::kernel(),
            vec![SessionMode::Terminal, SessionMode::Conversation]
        )],
    );
}

struct TriggerWorkspaceBytes {
    archive: Vec<u8>,
    reads: AtomicUsize,
}

struct MalformedTriggerWorkspace;

#[async_trait]
impl crate::SessionWorkspaceSource for MalformedTriggerWorkspace {
    async fn load_workspace(
        &self,
        _owner: &SubjectId,
        _origin: &crate::TriggerSessionOrigin,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        Err(StoreError::Malformed(
            "the workspace manifest did not match its object".into(),
        ))
    }
}

#[async_trait]
impl crate::SessionWorkspaceSource for TriggerWorkspaceBytes {
    async fn load_workspace(
        &self,
        _owner: &SubjectId,
        _origin: &crate::TriggerSessionOrigin,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some(self.archive.clone()))
    }
}

struct TriggerCredential;

#[async_trait]
impl crate::CredentialSource for TriggerCredential {
    async fn credential_for(
        &self,
        _session: &SessionId,
        _subject: &SubjectId,
        _services: &[crate::ServiceId],
        _caller_credential: Option<&crate::CallerCredential>,
    ) -> Result<Option<String>, StoreError> {
        Ok(Some("session-scoped-test-key".into()))
    }

    async fn release(&self, _session: &SessionId) -> Result<(), StoreError> {
        Ok(())
    }
}

fn trigger_session_service(
    archive: Vec<u8>,
) -> (
    Arc<SessionService>,
    Arc<FakeStore>,
    Arc<FakeAdapter>,
    Arc<crate::test_support::FakeProfileStore>,
    Arc<TriggerWorkspaceBytes>,
) {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    });
    let profiles = Arc::new(crate::test_support::FakeProfileStore::default());
    let workspaces = Arc::new(TriggerWorkspaceBytes {
        archive,
        reads: AtomicUsize::new(0),
    });
    let service = build_trigger_session_service(
        store.clone(),
        adapter.clone(),
        profiles.clone(),
        workspaces.clone(),
    );
    (service, store, adapter, profiles, workspaces)
}

fn build_trigger_session_service(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    profiles: Arc<crate::test_support::FakeProfileStore>,
    workspaces: Arc<dyn crate::SessionWorkspaceSource>,
) -> Arc<SessionService> {
    let environments = EnvironmentProfileService::new(
        profiles,
        crate::Baseline::default(),
        None,
        None,
        crate::AgentConfiguration {
            kernel: KernelId::new("codex"),
            mode: SessionMode::Conversation,
            ..Default::default()
        },
        crate::test_support::unconfigured_settings(),
    );
    let mut adapters: HashMap<KernelId, Arc<dyn layer3_adapter::KernelAdapter>> = HashMap::new();
    adapters.insert(KernelId::new("codex"), adapter.clone());
    let factory: ScreenFactory = Arc::new(|geometry| {
        Box::new(RecordingScreen {
            geometry,
            seen: Vec::new(),
        })
    });
    let service = SessionService::new(
        adapters,
        factory,
        store.clone(),
        SessionPorts {
            context: None,
            credentials: Some(Arc::new(TriggerCredential)),
            skills: None,
            workspaces: Some(workspaces),
            settings: None,
        },
        environments,
        crate::test_support::owned(),
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

#[tokio::test]
async fn trigger_session_creation_is_deterministic_and_hands_over_exact_workspace_bytes() {
    let archive = b"versioned repository archive\0binary".to_vec();
    let archive_sha256 = format!("{:x}", Sha256::digest(&archive));
    let launch = crate::trigger::tests::session_launch(&archive_sha256);
    let (service, store, adapter, profiles, workspaces) = trigger_session_service(archive.clone());

    let (first, concurrent_retry) = tokio::join!(
        service.ensure_trigger_session(&launch),
        service.ensure_trigger_session(&launch)
    );
    let created = first.expect("the Trigger session is durably created");
    assert_eq!(
        concurrent_retry
            .expect("a concurrent retry reconciles the same session")
            .id,
        created.id
    );
    let expected_id = format!("sesn_trg_{}", launch.id.0.replace('-', ""));
    assert_eq!(created.id.0, expected_id);
    assert_eq!(created.kernel, KernelId::new("codex"));
    assert_eq!(created.options.mode, SessionMode::Conversation);
    assert_eq!(
        created.options.permission,
        crate::PermissionMode::Autonomous
    );
    assert_eq!(created.options.credential, crate::CredentialMode::Delegated);
    assert_eq!(created.options.size, Some(launch.effective.resource_size));
    assert_eq!(
        created.granted_services,
        Some(vec![crate::ServiceId::new("service-1")])
    );

    for _ in 0..400 {
        let ready = store
            .get(&created.id)
            .await
            .unwrap()
            .is_some_and(|record| record.session.status == SessionStatus::Ready);
        if ready && service.trigger_credential_ready(&created.id).await {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(service.trigger_credential_ready(&created.id).await);
    let provisioned = adapter.last_spec.lock().await.clone().unwrap();
    let workspace = provisioned.workspace.expect("workspace was handed over");
    assert_eq!(workspace.archive, archive);
    assert_eq!(workspace.archive_sha256, archive_sha256);
    assert_eq!(workspace.source_revision, launch.source_sha);
    assert_eq!(workspace.epoch, 1);

    let public = serde_json::to_string(&created).unwrap();
    assert!(!public.contains("trigger_origin"));
    assert!(!public.contains("trigger-test/baseline.pack"));
    assert!(!public.contains("session-scoped-test-key"));

    let reconciled = service
        .ensure_trigger_session(&launch)
        .await
        .expect("the exact durable session is reusable");
    assert_eq!(reconciled.id, created.id);
    assert_eq!(adapter.provision_calls.load(Ordering::SeqCst), 1);
    assert_eq!(profiles.profiles.lock().await.len(), 1);

    let loaded_again = match service.workspace_for(&created.id).await {
        Ok(Some(workspace)) => workspace,
        Ok(None) => panic!("Trigger workspace remains required"),
        Err(error) => panic!(
            "replacement workspace source remains usable: {}",
            error.reason
        ),
    };
    assert_eq!(loaded_again.archive, workspace.archive);
    assert!(workspaces.reads.load(Ordering::SeqCst) >= 2);

    service.flush(&created.id).await;
    let durable_origin = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .trigger_origin
        .expect("a live-entry flush preserves internal Trigger provenance");
    assert_eq!(durable_origin.launch_id, launch.id);

    let restarted = build_trigger_session_service(
        store.clone(),
        adapter.clone(),
        profiles.clone(),
        workspaces.clone(),
    );
    let reclaimed = restarted.reconcile().await;
    assert_eq!(reclaimed.readopted_sessions, 1);
    let readopted_origin = restarted
        .entries
        .read()
        .await
        .get(&created.id)
        .and_then(|entry| entry.trigger_origin.clone())
        .expect("readoption restores internal Trigger provenance");
    assert_eq!(readopted_origin, durable_origin);
    restarted.flush(&created.id).await;
    assert_eq!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .trigger_origin
            .unwrap(),
        durable_origin
    );
}

#[tokio::test]
async fn trigger_managed_profiles_and_occupied_session_identities_fail_closed() {
    let archive = b"archive".to_vec();
    let archive_sha256 = format!("{:x}", Sha256::digest(&archive));
    let launch = crate::trigger::tests::session_launch(&archive_sha256);
    let (service, store, _adapter, profiles, _workspaces) = trigger_session_service(archive);
    let created = service.ensure_trigger_session(&launch).await.unwrap();
    let profile_id = created.environment_profile.clone().unwrap();
    let owner = user("trigger-contributor");

    assert_eq!(
        service
            .environments()
            .delete(&owner, &profile_id)
            .await
            .unwrap_err(),
        CoreError::Forbidden
    );

    let original_profile = profiles
        .profiles
        .lock()
        .await
        .get(&profile_id.0)
        .cloned()
        .unwrap();
    profiles
        .profiles
        .lock()
        .await
        .get_mut(&profile_id.0)
        .unwrap()
        .name = "occupied by different configuration".into();
    let profile_error = service
        .ensure_trigger_session(&launch)
        .await
        .expect_err("a changed deterministic profile is never overwritten");
    assert!(matches!(profile_error, CoreError::InvalidRequest(_)));
    assert_eq!(
        profiles
            .profiles
            .lock()
            .await
            .get(&profile_id.0)
            .unwrap()
            .name,
        "occupied by different configuration"
    );
    profiles
        .profiles
        .lock()
        .await
        .insert(profile_id.0.clone(), original_profile);
    let current = service
        .environments()
        .get(&owner, &profile_id)
        .await
        .unwrap();
    assert_eq!(
        service
            .environments()
            .update(&owner, &profile_id, current.to_spec())
            .await
            .unwrap_err(),
        CoreError::Forbidden
    );

    let mut corrupted = store.get(&created.id).await.unwrap().unwrap();
    corrupted.trigger_origin.as_mut().unwrap().revision_digest = "f".repeat(64);
    store
        .commit_session_idempotent("corrupt-trigger-origin", &corrupted, &[])
        .await
        .unwrap();
    let error = service
        .ensure_trigger_session(&launch)
        .await
        .expect_err("an occupied different identity is never overwritten");
    assert!(matches!(error, CoreError::InvalidRequest(_)));
    assert_eq!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .trigger_origin
            .unwrap()
            .revision_digest,
        "f".repeat(64)
    );
}

#[tokio::test]
async fn malformed_trigger_workspace_authority_is_not_retried() {
    let archive = b"archive".to_vec();
    let archive_sha256 = format!("{:x}", Sha256::digest(&archive));
    let launch = crate::trigger::tests::session_launch(&archive_sha256);
    let (service, store, adapter, profiles, _workspaces) = trigger_session_service(archive);
    let created = service.ensure_trigger_session(&launch).await.unwrap();
    let malformed = build_trigger_session_service(
        store,
        adapter,
        profiles,
        Arc::new(MalformedTriggerWorkspace),
    );

    let error = malformed
        .workspace_for(&created.id)
        .await
        .expect_err("malformed durable authority is a permanent integrity failure");
    assert!(!error.retryable);
    assert!(error.reason.contains("malformed"));
}

#[tokio::test]
async fn an_interface_that_only_holds_conversations_can_create_one() {
    // The plane a caller asked for was discarded along with everything else in
    // `options`, which is filled in from the profile or the deployment. On a
    // deployment defaulting to terminal that meant AG-UI created a terminal
    // session and then refused it for lacking the conversation ability it had
    // just asked for — so the priority-1 interface could not create a session
    // it could use, unless the caller had hand-made a profile first.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: crate::conversation_support::conversing(),
        ..FakeAdapter::default()
    });
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Terminal,
            ..Default::default()
        }),
    );
    let alice = user("alice");

    let created = svc
        .create_session(
            &alice,
            SessionSpec {
                requested_mode: Some(crate::options::SessionMode::Conversation),
                ..spec()
            },
        )
        .await
        .expect("created");

    assert_eq!(
        created.options.mode,
        crate::options::SessionMode::Conversation
    );
    assert!(
        created.capabilities.conversation,
        "and it is granted the ability, or the very next call refuses it"
    );
}

#[tokio::test]
async fn a_caller_who_says_nothing_still_gets_the_deployments_default() {
    // The narrowness matters. An interface with no way to express a plane must
    // keep getting what it always got.
    let store = Arc::new(FakeStore::default());
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Terminal,
            ..Default::default()
        }),
    );
    let alice = user("alice");

    let created = svc
        .create_session(
            &alice,
            SessionSpec {
                requested_mode: None,
                ..spec()
            },
        )
        .await
        .expect("created");

    assert_eq!(created.options.mode, crate::options::SessionMode::Terminal);
}
