//! Durable skill-pack replay and lifecycle cleanup.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use layer3_adapter::KernelAdapter;
use tokio::sync::{Mutex, Notify};

use crate::commit_fault_support::CommitFaultStore;
use crate::coordination::{SessionBoundary, SessionOperation};
use crate::event::EventKind;
use crate::store::{
    SessionStore, SkillPack, SkillPackState, SkillPackStore, SkillSource, StoreError,
};
use crate::test_support::{
    await_settled, service_on_version_with_skill_store, service_on_version_with_skills,
    service_over, spec, user, FakeAdapter, FakeStore,
};
use crate::{SessionId, SessionStatus, SkillId};

#[derive(Default)]
struct MemorySkillPacks {
    objects: Mutex<HashMap<String, Vec<SkillPack>>>,
    puts: AtomicUsize,
    gets: AtomicUsize,
    deletes: AtomicUsize,
    fail_reads: AtomicBool,
    fail_writes: AtomicUsize,
    fail_deletes: AtomicUsize,
    pause_next_put: AtomicBool,
    put_entered: Notify,
    resume_put: Notify,
    pause_next_delete: AtomicBool,
    delete_entered: Notify,
    resume_delete: Notify,
}

impl MemorySkillPacks {
    fn pause_next_put(&self) {
        assert!(!self.pause_next_put.swap(true, Ordering::SeqCst));
    }

    async fn wait_until_put_paused(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.put_entered.notified(),
        )
        .await
        .expect("skill pack PUT reached the store");
    }

    fn resume_put(&self) {
        self.resume_put.notify_one();
    }

    fn pause_next_delete(&self) {
        assert!(!self.pause_next_delete.swap(true, Ordering::SeqCst));
    }

    async fn wait_until_delete_paused(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.delete_entered.notified(),
        )
        .await
        .expect("skill pack DELETE reached the store");
    }

    fn resume_delete(&self) {
        self.resume_delete.notify_one();
    }
}

#[async_trait::async_trait]
impl SkillPackStore for MemorySkillPacks {
    async fn put(&self, id: &SessionId, packs: &[SkillPack]) -> Result<(), StoreError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_writes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(StoreError::Unavailable(
                "fixture store is unavailable".into(),
            ));
        }
        if self.pause_next_put.swap(false, Ordering::SeqCst) {
            self.put_entered.notify_one();
            self.resume_put.notified().await;
        }
        self.objects
            .lock()
            .await
            .insert(id.0.clone(), packs.to_vec());
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<Vec<SkillPack>>, StoreError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        if self.fail_reads.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable(
                "fixture store is unavailable".into(),
            ));
        }
        Ok(self.objects.lock().await.get(&id.0).cloned())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_deletes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(StoreError::Unavailable(
                "fixture cleanup store is unavailable".into(),
            ));
        }
        if self.pause_next_delete.swap(false, Ordering::SeqCst) {
            self.delete_entered.notify_one();
            self.resume_delete.notified().await;
        }
        self.objects.lock().await.remove(&id.0);
        Ok(())
    }
}

struct CountingSkills {
    calls: AtomicUsize,
    refuses: bool,
    version: &'static str,
    requests: Mutex<Vec<(bool, Vec<SkillId>)>>,
}

impl CountingSkills {
    fn working() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            refuses: false,
            version: "7.3",
            requests: Mutex::new(Vec::new()),
        }
    }

    fn at_version(version: &'static str) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            refuses: false,
            version,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn refusing() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            refuses: true,
            version: "7.3",
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl SkillSource for CountingSkills {
    async fn packs_for(
        &self,
        caller_credential: Option<&str>,
        skills: &[SkillId],
    ) -> Result<Vec<SkillPack>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .await
            .push((caller_credential.is_some(), skills.to_vec()));
        if self.refuses {
            return Err("the registry refused this identity".into());
        }
        Ok(skills
            .iter()
            .map(|skill| SkillPack {
                name: skill.0.clone(),
                version: Some(self.version.into()),
                files: vec![
                    ("SKILL.md".into(), b"# Durable fixture".to_vec()),
                    ("references/checks.txt".into(), b"one\ntwo\n".to_vec()),
                ],
            })
            .collect())
    }
}

struct EmptySkills;

#[async_trait::async_trait]
impl SkillSource for EmptySkills {
    async fn packs_for(
        &self,
        _caller_credential: Option<&str>,
        _skills: &[SkillId],
    ) -> Result<Vec<SkillPack>, String> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn creation_atomically_freezes_the_caller_visible_packs() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    packs.fail_writes.store(100, Ordering::SeqCst);
    let source = Arc::new(CountingSkills::working());
    let service = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::new(FakeAdapter::default()),
        "runtime:1",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        packs,
    );
    let owner = user("creation-custody-owner");
    let mut request = spec();
    request.caller_credential = Some("delegated-creation-custody".into());

    let created = service
        .create_session(&owner, request)
        .await
        .expect("the exact packs are resolved before creation");
    let record = sessions
        .get(&created.id)
        .await
        .expect("the created session is readable")
        .expect("the created session is durable");
    let SkillPackState::Staged(staged) = record.skill_packs else {
        panic!("creation must retain exact staged bytes until object custody succeeds");
    };

    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].name, "a-mandated-skill");
    assert_eq!(staged[0].version.as_deref(), Some("7.3"));
    assert_eq!(
        record.session.granted_skills,
        Some(vec![crate::environment::GrantedSkill {
            id: SkillId::new("a-mandated-skill"),
            version: Some("7.3".into()),
        }])
    );
    assert_eq!(
        source.requests.lock().await.as_slice(),
        &[(true, vec![SkillId::new("a-mandated-skill")])],
        "creation must resolve once with the caller's delegated authority"
    );
}

#[tokio::test]
async fn a_creation_refused_by_the_skill_source_leaves_no_session() {
    let sessions = Arc::new(FakeStore::default());
    let source = Arc::new(CountingSkills::refusing());
    let service = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::new(FakeAdapter::default()),
        "runtime:1",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        Arc::new(MemorySkillPacks::default()),
    );
    let mut request = spec();
    request.caller_credential = Some("delegated-refused-skill".into());

    assert!(matches!(
        service
            .create_session(&user("refused-skill-owner"), request)
            .await,
        Err(crate::CoreError::SkillCatalogUnavailable(_))
    ));
    assert!(sessions.sessions.lock().await.is_empty());
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn permanent_initial_provision_refusal_cleans_up_skill_pack_custody() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_at_capacity: true,
        ..FakeAdapter::default()
    });
    let service = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let owner = user("permanent-start-refusal-owner");

    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    let settled = await_settled(&service, &owner, &created.id).await;

    assert_eq!(settled.status, SessionStatus::Terminated);
    let terminal = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(terminal.skill_packs, SkillPackState::Cleaned);
    assert!(!packs.objects.lock().await.contains_key(&created.id.0));
    assert_eq!(packs.puts.load(Ordering::SeqCst), 1);
    assert_eq!(packs.deletes.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.provision_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn creation_without_skill_pack_custody_is_refused_before_provisioning() {
    let sessions = Arc::new(FakeStore::default());
    let source = Arc::new(CountingSkills::working());
    let adapter = Arc::new(FakeAdapter::default());
    let service = service_on_version_with_skills(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::clone(&source) as Arc<dyn SkillSource>,
    );

    service
        .create_session(&user("missing-skill-custody-owner"), spec())
        .await
        .expect_err("nonempty skill bytes require durable custody");

    assert!(sessions.sessions.lock().await.is_empty());
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn empty_skill_resolution_needs_no_object_custody() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let service = service_on_version_with_skills(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(EmptySkills),
    );
    let owner = user("empty-skill-resolution-owner");

    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    assert_eq!(
        await_settled(&service, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    let ready = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(ready.session.granted_skills, Some(Vec::new()));
    assert_eq!(ready.skill_packs, SkillPackState::Staged(Vec::new()));
    assert_eq!(adapter.provision_calls.load(Ordering::SeqCst), 1);

    service
        .terminate_session(&owner, &created.id)
        .await
        .expect("empty custody terminates without an object store");
    assert_eq!(
        sessions
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .skill_packs,
        SkillPackState::Cleaned
    );
}

#[tokio::test]
async fn a_storage_retry_reuses_the_registry_bytes_already_fetched() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    packs.fail_writes.store(2, Ordering::SeqCst);
    let source = Arc::new(CountingSkills::working());
    let adapter = Arc::new(FakeAdapter::default());
    let service = service_on_version_with_skill_store(
        sessions,
        adapter,
        "runtime:1",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let owner = user("write-retry-owner");
    let mut request = spec();
    request.caller_credential = Some("delegated-write-retry".into());
    let created = service
        .create_session(&owner, request)
        .await
        .expect("created");

    assert_eq!(
        await_settled(&service, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(packs.puts.load(Ordering::SeqCst), 3);
    assert!(packs.objects.lock().await.contains_key(&created.id.0));
}

#[tokio::test]
async fn creation_fetches_once_and_rotation_replays_without_the_registry() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let source = Arc::new(CountingSkills::working());
    let adapter = Arc::new(FakeAdapter::default());
    // Exercise provider retry too: one logical creation must still resolve the
    // registry once and replay the same in-memory/durable bytes thereafter.
    adapter.fail_first.store(2, Ordering::SeqCst);
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let owner = user("skill-owner");
    let mut request = spec();
    request.caller_credential = Some("delegated-fixture".into());
    let created = first
        .create_session(&owner, request)
        .await
        .expect("created");
    assert_eq!(
        await_settled(&first, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        packs.puts.load(Ordering::SeqCst),
        1,
        "a confirmed durable write is not repeated for provider retries"
    );
    let originally_installed = adapter
        .last_spec
        .lock()
        .await
        .clone()
        .expect("a runtime was provisioned")
        .skills;
    assert_eq!(originally_installed.len(), 1);
    assert_eq!(
        packs.objects.lock().await[&created.id.0][0]
            .version
            .as_deref(),
        Some("7.3")
    );

    let unavailable_source = Arc::new(CountingSkills::refusing());
    let newer = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::clone(&unavailable_source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    newer.reconcile().await;
    assert_eq!(
        await_settled(&newer, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    assert_eq!(
        unavailable_source.calls.load(Ordering::SeqCst),
        0,
        "a stored pack makes the registry irrelevant to rotation"
    );
    let replayed = adapter
        .last_spec
        .lock()
        .await
        .clone()
        .expect("the replacement was provisioned")
        .skills;
    assert_eq!(replayed.len(), originally_installed.len());
    assert_eq!(replayed[0].name, originally_installed[0].name);
    assert_eq!(replayed[0].files, originally_installed[0].files);

    let lost = sessions
        .get(&created.id)
        .await
        .expect("the rotated session is readable")
        .expect("the rotated session exists")
        .runtime_id
        .expect("the rotated runtime is durable");
    adapter.stop_serving(&lost).await;
    let recovery = newer.reconcile().await;
    assert_eq!(recovery.reprovisioned_sessions, 1);
    assert_eq!(
        await_settled(&newer, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    assert_eq!(unavailable_source.calls.load(Ordering::SeqCst), 0);
    let recovered = adapter
        .last_spec
        .lock()
        .await
        .clone()
        .expect("runtime-loss recovery provisioned a replacement")
        .skills;
    assert_eq!(recovered.len(), replayed.len());
    assert_eq!(recovered[0].name, replayed[0].name);
    assert_eq!(recovered[0].files, replayed[0].files);
}

#[tokio::test]
async fn compatibility_plane_change_replays_stored_skills_without_the_registry() {
    let sessions = Arc::new(FakeStore::default());
    sessions.disable_coordination();
    let packs = Arc::new(MemorySkillPacks::default());
    let mut capable = FakeAdapter::default();
    capable.capabilities.conversation = true;
    let adapter = Arc::new(capable);
    let owner = user("compatibility-skill-owner");
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    let before = sessions.get(&created.id).await.unwrap().unwrap();
    let h1 = before.runtime_id.clone().expect("H1 is durable");
    let installed = adapter.last_spec.lock().await.clone().unwrap().skills;

    let unavailable_source = Arc::new(CountingSkills::refusing());
    let moving = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::clone(&unavailable_source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    moving.reconcile().await;
    moving
        .change_plane(
            &owner,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect("the compatibility handover starts");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let current = sessions.get(&created.id).await.unwrap().unwrap();
            if current.session.status == SessionStatus::Ready
                && current.session.options.mode == crate::options::SessionMode::Conversation
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the compatibility handover settles");

    let moved = sessions.get(&created.id).await.unwrap().unwrap();
    assert_ne!(moved.runtime_id.as_deref(), Some(h1.as_str()));
    assert!(matches!(moved.skill_packs, SkillPackState::Stored { .. }));
    assert_eq!(unavailable_source.calls.load(Ordering::SeqCst), 0);
    let replayed = adapter.last_spec.lock().await.clone().unwrap().skills;
    assert_eq!(replayed.len(), installed.len());
    assert_eq!(replayed[0].name, installed[0].name);
    assert_eq!(replayed[0].files, installed[0].files);
}

#[tokio::test]
async fn legacy_handover_readopts_h1_without_publishing_h2_skills_and_retries_exact_bytes() {
    let sessions = Arc::new(FakeStore::default());
    sessions.disable_coordination();
    let packs = Arc::new(MemorySkillPacks::default());
    let mut capable = FakeAdapter::default();
    capable.capabilities.conversation = true;
    let adapter = Arc::new(capable);
    let owner = user("legacy-h1-skill-handover-owner");
    let original_source = Arc::new(CountingSkills::at_version("1.0"));
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        original_source,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    let mut legacy = sessions.get(&created.id).await.unwrap().unwrap();
    let h1 = legacy.runtime_id.clone().expect("H1 is durable");
    let old_grant = legacy
        .session
        .granted_skills
        .clone()
        .expect("H1 has its installed grant");
    assert_eq!(old_grant[0].version.as_deref(), Some("1.0"));
    legacy.skill_packs = SkillPackState::Legacy;
    sessions.seed(&legacy).await.expect("seed legacy custody");
    packs.objects.lock().await.remove(&created.id.0);

    let candidate_source = Arc::new(CountingSkills::at_version("2.0"));
    let moving = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::clone(&candidate_source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    assert_eq!(moving.reconcile().await.readopted_sessions, 1);
    adapter.fail_terminations(1);
    assert!(moving
        .change_plane(
            &owner,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .is_err());

    let interrupted = sessions.get(&created.id).await.unwrap().unwrap();
    let SkillPackState::LegacyHandover {
        source_runtime_id,
        source_generation,
        packs: candidate,
        stored_digest,
    } = interrupted.skill_packs
    else {
        panic!("the interrupted handover did not retain both skill truths");
    };
    assert_eq!(source_runtime_id, h1);
    assert_eq!(source_generation, 0);
    assert!(stored_digest.is_some());
    assert_eq!(candidate[0].version.as_deref(), Some("2.0"));
    assert_eq!(interrupted.session.granted_skills, Some(old_grant.clone()));
    assert_eq!(candidate_source.calls.load(Ordering::SeqCst), 1);

    assert_eq!(moving.reconcile().await.readopted_sessions, 1);
    let readopted = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(readopted.session.status, SessionStatus::Ready);
    assert_eq!(readopted.runtime_id.as_deref(), Some(h1.as_str()));
    assert_eq!(readopted.rotation_claim, None);
    assert_eq!(readopted.pending_handover, None);
    assert_eq!(readopted.skill_packs, SkillPackState::Legacy);
    assert_eq!(readopted.session.granted_skills, Some(old_grant));
    assert_eq!(
        packs.objects.lock().await[&created.id.0],
        candidate,
        "H1 settlement must not delete the exact H2 candidate object"
    );

    moving
        .change_plane(
            &owner,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect("the surviving H1 retries its handover");
    await_settled(&moving, &owner, &created.id).await;
    let moved = sessions.get(&created.id).await.unwrap().unwrap();
    assert_ne!(moved.runtime_id.as_deref(), Some(h1.as_str()));
    assert_eq!(moved.session.status, SessionStatus::Ready);
    assert_eq!(
        moved.session.granted_skills.as_ref().unwrap()[0]
            .version
            .as_deref(),
        Some("2.0")
    );
    assert!(matches!(moved.skill_packs, SkillPackState::Stored { .. }));
    assert_eq!(candidate_source.calls.load(Ordering::SeqCst), 1);
    let installed = adapter.last_spec.lock().await.clone().unwrap().skills;
    assert_eq!(installed[0].files, candidate[0].files);
}

#[tokio::test]
async fn legacy_handover_readopts_switched_h2_with_its_exact_grant_and_recovery_bytes() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("legacy-h2-skill-handover-owner");
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::at_version("1.0")),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    let mut legacy = sessions.get(&created.id).await.unwrap().unwrap();
    let h1 = legacy.runtime_id.clone().expect("H1 is durable");
    let old_grant = legacy.session.granted_skills.clone();
    legacy.skill_packs = SkillPackState::Legacy;
    sessions
        .commit_session_idempotent("seed-legacy-h2-custody", &legacy, &[])
        .await
        .expect("seed legacy custody");
    packs.objects.lock().await.remove(&created.id.0);
    let legacy = sessions.get(&created.id).await.unwrap().unwrap();

    let candidate_source = Arc::new(CountingSkills::at_version("2.0"));
    let moving = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::clone(&candidate_source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let mut lease = moving
        .acquire_runtime_lease_at(&legacy, SessionOperation::Handover)
        .await
        .expect("handover lease acquisition succeeds")
        .expect("coordination is enabled");
    let prepared = moving
        .prepare_skill_packs_for_handover(&legacy.session, Some(&lease))
        .await
        .unwrap_or_else(|failure| {
            panic!(
                "the exact H2 candidate is prepared once: {}",
                failure.reason
            )
        });
    assert_eq!(candidate_source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(prepared.packs[0].version.as_deref(), Some("2.0"));
    assert!(prepared.stored_digest.is_some());

    let source_generation = lease.boundary.authority.generation;
    let mut rotating = legacy.clone();
    rotating.session.status = SessionStatus::Rotating;
    rotating.rotation_claim = Some("rotn_legacy_h2_crash_boundary".into());
    rotating.skill_packs = SkillPackState::LegacyHandover {
        source_runtime_id: h1.clone(),
        source_generation,
        packs: prepared.packs.clone(),
        stored_digest: prepared.stored_digest,
    };
    moving
        .mark_handover_durably(&mut rotating, &legacy, Vec::new())
        .await
        .expect("the handover candidate and claim become durable together");
    let rotating_boundary = SessionBoundary::for_record(&rotating, source_generation);
    moving
        .advance_runtime_lease(&mut lease, &rotating_boundary)
        .await
        .expect("the handover lease advances to Rotating");

    let reservation = sessions
        .reserve_runtime_generation(&lease, "legacy-h2-crash-reservation")
        .await
        .expect("H2 generation is reserved");
    let mut h2_spec = adapter
        .last_spec
        .lock()
        .await
        .clone()
        .expect("H1 was provisioned from a complete spec");
    h2_spec.generation = Some(reservation.generation);
    h2_spec.reservation_token = Some(reservation.token.clone());
    h2_spec.skills = prepared
        .packs
        .iter()
        .map(|pack| layer3_adapter::SkillPackage {
            name: pack.name.clone(),
            files: pack.files.clone(),
        })
        .collect();
    let h2 = adapter
        .provision(&h2_spec)
        .await
        .expect("H2 receives the exact candidate bytes");
    let candidate = sessions
        .register_runtime_candidate(&lease, &reservation, &h2.0)
        .await
        .expect("H2 is registered before authority switches");
    sessions
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .expect("H2 becomes durable authority");
    sessions
        .release_session_lease(&lease)
        .await
        .expect("the crashed writer's lease is released by the fixture");

    let switched = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(switched.session.status, SessionStatus::Rotating);
    assert_eq!(switched.runtime_id.as_deref(), Some(h2.0.as_str()));
    assert_eq!(switched.session.granted_skills, old_grant);
    assert!(matches!(
        switched.skill_packs,
        SkillPackState::LegacyHandover { .. }
    ));

    let restarted = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::clone(&candidate_source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    assert_eq!(restarted.reconcile().await.readopted_sessions, 1);
    await_settled(&restarted, &owner, &created.id).await;
    let settled = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(settled.session.status, SessionStatus::Ready);
    assert_eq!(settled.runtime_id.as_deref(), Some(h2.0.as_str()));
    assert_eq!(settled.rotation_claim, None);
    assert_eq!(settled.pending_handover, None);
    assert_eq!(
        settled.session.granted_skills.as_ref().unwrap()[0]
            .version
            .as_deref(),
        Some("2.0")
    );
    assert!(matches!(settled.skill_packs, SkillPackState::Stored { .. }));
    assert_eq!(candidate_source.calls.load(Ordering::SeqCst), 1);

    adapter.stop_serving(&h2.0).await;
    assert_eq!(restarted.reconcile().await.reprovisioned_sessions, 1);
    await_settled(&restarted, &owner, &created.id).await;
    assert_eq!(candidate_source.calls.load(Ordering::SeqCst), 1);
    let replayed = adapter.last_spec.lock().await.clone().unwrap().skills;
    assert_eq!(replayed[0].files, prepared.packs[0].files);
}

#[tokio::test]
async fn inline_legacy_handover_candidate_cleans_without_a_pack_store() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("inline-legacy-handover-cleanup-owner");
    let service = service_over(
        Arc::clone(&sessions) as Arc<dyn SessionStore>,
        Arc::clone(&adapter),
    );
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    let mut terminal = sessions.get(&created.id).await.unwrap().unwrap();
    let source_runtime_id = terminal.runtime_id.take().expect("H1 is durable");
    terminal.session.status = SessionStatus::Terminated;
    terminal.skill_packs = SkillPackState::LegacyHandover {
        source_runtime_id,
        source_generation: 0,
        packs: vec![SkillPack {
            name: "inline-candidate".into(),
            version: Some("1.0".into()),
            files: vec![("SKILL.md".into(), b"# Inline candidate".to_vec())],
        }],
        stored_digest: None,
    };
    sessions
        .seed(&terminal)
        .await
        .expect("seed terminal custody");
    let mut terminal = sessions.get(&created.id).await.unwrap().unwrap();

    service
        .finish_skill_pack_cleanup(&mut terminal, None)
        .await
        .expect("inline-only candidate has no external deletion debt");
    assert_eq!(terminal.skill_packs, SkillPackState::Cleaned);
    assert_eq!(
        sessions
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .skill_packs,
        SkillPackState::Cleaned
    );
}

#[tokio::test]
async fn a_missing_legacy_pack_recovers_only_the_frozen_grant() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let old_packs = Arc::new(MemorySkillPacks::default());
    let owner = user("legacy-skill-owner");
    let old = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&old_packs) as Arc<dyn SkillPackStore>,
    );
    let created = old.create_session(&owner, spec()).await.expect("created");
    await_settled(&old, &owner, &created.id).await;

    let mut legacy = sessions
        .get(&created.id)
        .await
        .expect("legacy session is readable")
        .expect("legacy session exists");
    let original_runtime = legacy.runtime_id.clone().expect("the old runtime is live");
    legacy.skill_packs = SkillPackState::Legacy;
    legacy.session.granted_skills = Some(vec![crate::environment::GrantedSkill {
        id: SkillId::new("frozen-private-skill"),
        version: Some("old-version".into()),
    }]);
    sessions
        .seed(&legacy)
        .await
        .expect("the legacy representation is installed");
    sessions.authorize_legacy_cleanup();
    adapter.generations.lock().await.remove(&original_runtime);
    old_packs.objects.lock().await.remove(&created.id.0);

    let packs = Arc::new(MemorySkillPacks::default());
    let source = Arc::new(CountingSkills::working());
    let newer = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    newer.reconcile().await;
    let settled = await_settled(&newer, &owner, &created.id).await;

    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        source.requests.lock().await.as_slice(),
        &[(false, vec![SkillId::new("frozen-private-skill")])],
        "legacy recovery must use the frozen IDs without borrowing caller authority"
    );
    assert_eq!(
        packs.objects.lock().await[&created.id.0][0].name,
        "frozen-private-skill"
    );
    let recovered = sessions
        .get(&created.id)
        .await
        .expect("recovered session is readable")
        .expect("recovered session exists");
    assert!(matches!(
        recovered.skill_packs,
        SkillPackState::Stored { .. }
    ));
    assert_eq!(
        recovered.session.granted_skills,
        Some(vec![crate::environment::GrantedSkill {
            id: SkillId::new("frozen-private-skill"),
            version: Some("7.3".into()),
        }]),
        "Ready publishes the grant derived from the exact fallback bytes installed in H2"
    );
}

#[tokio::test]
async fn a_legacy_session_without_a_frozen_grant_uses_the_profile_once() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let old_packs = Arc::new(MemorySkillPacks::default());
    let owner = user("unknown-legacy-skill-owner");
    let old = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&old_packs) as Arc<dyn SkillPackStore>,
    );
    let created = old.create_session(&owner, spec()).await.expect("created");
    await_settled(&old, &owner, &created.id).await;

    let mut legacy = sessions
        .get(&created.id)
        .await
        .expect("legacy session is readable")
        .expect("legacy session exists");
    let original_runtime = legacy.runtime_id.clone().expect("the old runtime is live");
    legacy.skill_packs = SkillPackState::Legacy;
    legacy.session.granted_skills = None;
    sessions
        .seed(&legacy)
        .await
        .expect("the legacy representation is installed");
    sessions.authorize_legacy_cleanup();
    adapter.generations.lock().await.remove(&original_runtime);
    old_packs.objects.lock().await.remove(&created.id.0);

    let packs = Arc::new(MemorySkillPacks::default());
    let source = Arc::new(CountingSkills::working());
    let newer = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    newer.reconcile().await;
    let settled = await_settled(&newer, &owner, &created.id).await;

    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        source.requests.lock().await.as_slice(),
        &[(false, vec![SkillId::new("a-mandated-skill")])]
    );
    assert_eq!(
        packs.objects.lock().await[&created.id.0][0].name,
        "a-mandated-skill"
    );
    let recovered = sessions
        .get(&created.id)
        .await
        .expect("recovered session is readable")
        .expect("recovered session exists");
    assert!(matches!(
        recovered.skill_packs,
        SkillPackState::Stored { .. }
    ));
    assert_eq!(
        recovered.session.granted_skills,
        Some(vec![crate::environment::GrantedSkill {
            id: SkillId::new("a-mandated-skill"),
            version: Some("7.3".into()),
        }])
    );
    assert_ne!(
        recovered.runtime_id.as_deref(),
        Some(original_runtime.as_str())
    );
    newer.reconcile().await;
    assert!(adapter
        .terminated
        .lock()
        .await
        .iter()
        .any(|runtime| runtime == &original_runtime));
}

#[tokio::test]
async fn an_existing_legacy_object_replaces_its_stale_old_grant() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("legacy-object-owner");
    let old_packs = Arc::new(MemorySkillPacks::default());
    let old = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        old_packs,
    );
    let created = old.create_session(&owner, spec()).await.expect("created");
    await_settled(&old, &owner, &created.id).await;
    let mut legacy = sessions.get(&created.id).await.unwrap().unwrap();
    legacy.skill_packs = SkillPackState::Legacy;
    legacy.session.granted_skills = Some(vec![crate::environment::GrantedSkill {
        id: SkillId::new("stale-old-skill"),
        version: Some("1.0".into()),
    }]);
    sessions.seed(&legacy).await.expect("seed legacy row");

    let exact = SkillPack {
        name: "object-authoritative-skill".into(),
        version: Some("9.4".into()),
        files: vec![("SKILL.md".into(), b"# Object authority".to_vec())],
    };
    let packs = Arc::new(MemorySkillPacks::default());
    packs
        .objects
        .lock()
        .await
        .insert(created.id.0.clone(), vec![exact.clone()]);
    let source = Arc::new(CountingSkills::refusing());
    let newer = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        packs,
    );

    newer.reconcile().await;
    assert_eq!(
        await_settled(&newer, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    let durable = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(
        durable.session.granted_skills,
        Some(vec![crate::environment::GrantedSkill {
            id: SkillId::new(exact.name.clone()),
            version: exact.version.clone(),
        }])
    );
    let installed = adapter.last_spec.lock().await.clone().unwrap().skills;
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].name, exact.name);
    assert_eq!(installed[0].files, exact.files);
}

#[tokio::test]
async fn legacy_registry_refusal_is_reused_across_provider_retries() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("legacy-refusal-retry-owner");
    let old = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::new(MemorySkillPacks::default()),
    );
    let created = old.create_session(&owner, spec()).await.expect("created");
    await_settled(&old, &owner, &created.id).await;
    let mut legacy = sessions.get(&created.id).await.unwrap().unwrap();
    legacy.skill_packs = SkillPackState::Legacy;
    legacy.session.granted_skills = Some(vec![crate::environment::GrantedSkill {
        id: SkillId::new("unavailable-legacy-skill"),
        version: Some("old".into()),
    }]);
    sessions.seed(&legacy).await.expect("seed legacy row");

    adapter.fail_first.store(2, Ordering::SeqCst);
    let calls_before = adapter.provision_calls.load(Ordering::SeqCst);
    let packs = Arc::new(MemorySkillPacks::default());
    let source = Arc::new(CountingSkills::refusing());
    let newer = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );

    newer.reconcile().await;
    assert_eq!(
        await_settled(&newer, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        adapter.provision_calls.load(Ordering::SeqCst) - calls_before,
        3
    );
    assert!(adapter
        .last_spec
        .lock()
        .await
        .as_ref()
        .unwrap()
        .skills
        .is_empty());
    assert_eq!(
        sessions
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .granted_skills,
        Some(Vec::new())
    );
    assert!(packs
        .objects
        .lock()
        .await
        .get(&created.id.0)
        .is_some_and(Vec::is_empty));
}

#[tokio::test]
async fn a_pack_that_disagrees_with_the_frozen_grant_cannot_replace_the_runtime() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let source = Arc::new(CountingSkills::working());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("mismatched-pack-owner");
    let old = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = old.create_session(&owner, spec()).await.expect("created");
    await_settled(&old, &owner, &created.id).await;
    let before = sessions
        .get(&created.id)
        .await
        .expect("session is readable")
        .expect("session exists");
    let original_runtime = before.runtime_id.clone().expect("the old runtime is live");
    packs.objects.lock().await.get_mut(&created.id.0).unwrap()[0].version = Some("8.0".into());

    let newer = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::new(CountingSkills::refusing()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    newer.reconcile().await;
    let after = sessions
        .get(&created.id)
        .await
        .expect("session is readable after refused rotation")
        .expect("session survives refused rotation");

    assert_eq!(after.session.status, SessionStatus::Ready);
    assert_eq!(after.session.granted_skills, before.session.granted_skills);
    assert_eq!(after.runtime_id.as_deref(), Some(original_runtime.as_str()));
    assert!(adapter.terminated.lock().await.is_empty());
}

async fn assert_stored_pack_corruption_preserves_h1(mutate: fn(&mut SkillPack)) {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("corrupt-pack-owner");
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    let before = sessions
        .get(&created.id)
        .await
        .expect("session is readable")
        .expect("session exists");
    let original_runtime = before.runtime_id.clone().expect("H1 is live");
    mutate(&mut packs.objects.lock().await.get_mut(&created.id.0).unwrap()[0]);

    let source = Arc::new(CountingSkills::refusing());
    let newer = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        packs,
    );
    newer.reconcile().await;
    let after = sessions
        .get(&created.id)
        .await
        .expect("session remains readable")
        .expect("session survives refused rotation");

    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    assert_eq!(after.session.status, SessionStatus::Ready);
    assert_eq!(after.session.granted_skills, before.session.granted_skills);
    assert_eq!(after.skill_packs, before.skill_packs);
    assert_eq!(after.runtime_id.as_deref(), Some(original_runtime.as_str()));
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn a_stored_pack_with_a_changed_path_cannot_replace_h1() {
    assert_stored_pack_corruption_preserves_h1(|pack| {
        pack.files[0].0 = "references/replaced.md".into();
    })
    .await;
}

#[tokio::test]
async fn a_stored_pack_with_a_changed_body_cannot_replace_h1() {
    assert_stored_pack_corruption_preserves_h1(|pack| {
        pack.files[0].1 = b"changed without a version bump".to_vec();
    })
    .await;
}

#[tokio::test]
async fn a_missing_stored_pack_cannot_replace_h1() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("missing-stored-pack-owner");
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    let before = sessions.get(&created.id).await.unwrap().unwrap();
    let h1 = before.runtime_id.clone().expect("H1 is durable");
    packs.objects.lock().await.remove(&created.id.0);
    let source = Arc::new(CountingSkills::refusing());
    let calls_before = adapter.provision_calls.load(Ordering::SeqCst);
    let newer = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        packs,
    );

    newer.reconcile().await;
    let after = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    assert_eq!(adapter.provision_calls.load(Ordering::SeqCst), calls_before);
    assert_eq!(after.session.status, SessionStatus::Ready);
    assert_eq!(after.runtime_id.as_deref(), Some(h1.as_str()));
    assert_eq!(after.skill_packs, before.skill_packs);
    assert_eq!(after.session.granted_skills, before.session.granted_skills);
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn a_slow_pack_put_keeps_remote_termination_out_of_handover() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("slow-put-handover-owner");
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    let mut staged = sessions.get(&created.id).await.unwrap().unwrap();
    let h1 = staged.runtime_id.clone().expect("H1 is durable");
    staged.skill_packs = SkillPackState::Staged(packs.objects.lock().await[&created.id.0].clone());
    sessions.seed(&staged).await.expect("seed staged custody");
    packs.objects.lock().await.remove(&created.id.0);
    packs.pause_next_put();

    let upgrading = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::new(CountingSkills::refusing()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let sweep = tokio::spawn({
        let upgrading = Arc::clone(&upgrading);
        async move { upgrading.reconcile().await }
    });
    packs.wait_until_put_paused().await;

    let remote = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::new(CountingSkills::refusing()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        remote.terminate_session(&owner, &created.id),
    )
    .await
    .expect("the remote lifecycle attempt receives the lease conflict");
    assert!(refused.is_err());
    let during = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(during.session.status, SessionStatus::Ready);
    assert_eq!(during.runtime_id.as_deref(), Some(h1.as_str()));
    assert_eq!(packs.deletes.load(Ordering::SeqCst), 0);

    packs.resume_put();
    sweep.await.expect("reconciliation task completes");
    assert_eq!(
        await_settled(&upgrading, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    let moved = sessions.get(&created.id).await.unwrap().unwrap();
    assert_ne!(moved.runtime_id.as_deref(), Some(h1.as_str()));
    assert!(matches!(moved.skill_packs, SkillPackState::Stored { .. }));
    assert!(packs.objects.lock().await.contains_key(&created.id.0));
}

#[tokio::test]
async fn an_expired_handover_lease_after_put_cannot_replace_h1() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("expired-put-lease-owner");
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    let mut staged = sessions.get(&created.id).await.unwrap().unwrap();
    let h1 = staged.runtime_id.clone().expect("H1 is durable");
    let exact = packs.objects.lock().await[&created.id.0].clone();
    first
        .persist_skill_pack_state_durably(&mut staged, SkillPackState::Staged(exact))
        .await
        .expect("the fixture restores staged custody");
    packs.objects.lock().await.remove(&created.id.0);
    packs.pause_next_put();
    let provisions_before = adapter.provision_calls.load(Ordering::SeqCst);

    let upgrading = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::new(CountingSkills::refusing()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let sweep = tokio::spawn({
        let upgrading = Arc::clone(&upgrading);
        async move { upgrading.reconcile().await }
    });
    packs.wait_until_put_paused().await;
    let lease = sessions
        .active_session_lease(&created.id)
        .await
        .expect("the PUT remains inside its handover lease");
    sessions
        .renew_session_lease(&lease, std::time::Duration::ZERO)
        .await
        .expect("the fixture expires the exact handover lease");
    packs.resume_put();
    sweep.await.expect("the expired sweep completes");

    let preserved = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(preserved.session.status, SessionStatus::Ready);
    assert_eq!(preserved.runtime_id.as_deref(), Some(h1.as_str()));
    assert!(matches!(preserved.skill_packs, SkillPackState::Staged(_)));
    assert_eq!(
        adapter.provision_calls.load(Ordering::SeqCst),
        provisions_before
    );
    assert!(adapter.terminated.lock().await.is_empty());
    assert!(packs.objects.lock().await.contains_key(&created.id.0));

    upgrading.reconcile().await;
    assert_eq!(
        await_settled(&upgrading, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    let recovered = sessions.get(&created.id).await.unwrap().unwrap();
    assert_ne!(recovered.runtime_id.as_deref(), Some(h1.as_str()));
    assert!(matches!(
        recovered.skill_packs,
        SkillPackState::Stored { .. }
    ));
}

#[tokio::test]
async fn a_store_outage_is_not_mistaken_for_legacy_absence() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let source = Arc::new(CountingSkills::working());
    let owner = user("outage-skill-owner");
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::clone(&source) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    let original_runtime = sessions
        .get(&created.id)
        .await
        .expect("session is readable")
        .expect("session exists")
        .runtime_id
        .expect("the old runtime is live");

    packs.fail_reads.store(true, Ordering::SeqCst);
    let fallback = Arc::new(CountingSkills::refusing());
    let newer = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::clone(&fallback) as Arc<dyn SkillSource>,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    newer.reconcile().await;
    let settled = await_settled(&newer, &owner, &created.id).await;

    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(fallback.calls.load(Ordering::SeqCst), 0);
    assert!(packs.objects.lock().await.contains_key(&created.id.0));
    assert_eq!(
        sessions
            .get(&created.id)
            .await
            .expect("session remains readable")
            .expect("session remains present")
            .runtime_id
            .as_deref(),
        Some(original_runtime.as_str())
    );
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn termination_replays_skill_cleanup_safely() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let source = Arc::new(CountingSkills::working());
    let owner = user("ended-skill-owner");
    let service = service_on_version_with_skill_store(
        sessions,
        adapter,
        "runtime:1",
        source,
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    assert!(packs.objects.lock().await.contains_key(&created.id.0));

    service
        .terminate_session(&owner, &created.id)
        .await
        .expect("first termination succeeds");
    service
        .terminate_session(&owner, &created.id)
        .await
        .expect("replayed termination succeeds");

    assert!(!packs.objects.lock().await.contains_key(&created.id.0));
    assert_eq!(
        packs.deletes.load(Ordering::SeqCst),
        1,
        "a durable Cleaned acknowledgement makes later terminal observations side-effect free"
    );
    let record = service
        .store
        .get(&created.id)
        .await
        .expect("terminal record is readable")
        .expect("terminal record remains owned");
    assert_eq!(record.skill_packs, SkillPackState::Cleaned);
}

#[tokio::test]
async fn terminal_repair_inherits_a_racing_empty_pack_cleanup_acknowledgement() {
    let sessions = Arc::new(FakeStore::default());
    let cleanup_store = Arc::new(CommitFaultStore::new(
        Arc::clone(&sessions),
        |record, events| {
            events.is_empty()
                && record.session.status.is_terminal()
                && record.skill_packs == SkillPackState::Cleaned
        },
    ));
    let repair_store = Arc::new(CommitFaultStore::new(
        Arc::clone(&sessions),
        |record, events| {
            record.session.status == SessionStatus::Terminated
                && record.pending_handover.is_none()
                && events.iter().any(|event| {
                    matches!(
                        event.kind,
                        EventKind::StatusChanged {
                            status: SessionStatus::Terminated,
                            ..
                        }
                    )
                })
        },
    ));
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("terminal-cleanup-rebase-owner");
    let original = service_over(cleanup_store.clone(), Arc::clone(&adapter));
    let created = original
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&original, &owner, &created.id).await;

    cleanup_store.pause_next();
    original
        .terminate_session(&owner, &created.id)
        .await
        .expect("the first termination reaches its terminal boundary");
    cleanup_store.wait_until_held().await;

    let mut residual = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(residual.session.status, SessionStatus::Terminated);
    assert_eq!(residual.runtime_id, None);
    assert_eq!(residual.rotation_claim, None);
    assert_eq!(residual.skill_packs, SkillPackState::Staged(Vec::new()));
    let frozen_grant = residual.session.granted_skills.clone();
    residual.context_ref = Some("context-terminal-cleanup-rebase".into());
    residual.pending_handover = Some(crate::store::PendingHandover {
        claim: "rotn_terminal_cleanup_rebase".into(),
        snapshot_ref: format!("sessions/{}/handover.snapshot", created.id.0),
        snapshot_at: "2026-08-27T02:03:04Z".into(),
        rotations_from: residual.session.rotations,
        rotations_to: residual.session.rotations.saturating_add(1),
        target: crate::store::PendingHandoverTarget::RuntimeVersion {
            from_runtime_version: residual.session.runtime_version.clone(),
            runtime_version: "runtime:unconsumed".into(),
        },
    });
    residual.revision = sessions
        .commit_session_idempotent("test-terminal-cleanup-rebase-residual", &residual, &[])
        .await
        .expect("the interrupted terminal descriptor is durable");

    repair_store.pause_next();
    let repairing = tokio::spawn({
        let restarted = service_over(repair_store.clone(), Arc::clone(&adapter));
        let owner = owner.clone();
        let id = created.id.clone();
        async move { restarted.terminate_session(&owner, &id).await }
    });
    repair_store.wait_until_held().await;

    cleanup_store.release();
    let cleaned = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let record = sessions.get(&created.id).await.unwrap().unwrap();
            if record.skill_packs == SkillPackState::Cleaned {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("empty custody cleanup rebases over the injected descriptor");
    assert!(cleaned.revision > residual.revision);
    assert_eq!(cleaned.pending_handover, residual.pending_handover);
    assert_eq!(cleaned.session.last_seq, residual.session.last_seq);
    assert_eq!(cleaned.context_ref, residual.context_ref);
    let history_before_repair = sessions
        .events_since(&created.id, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|event| (event.seq, format!("{:?}", event.kind)))
        .collect::<Vec<_>>();

    repair_store.release();
    repairing
        .await
        .expect("the restarted termination task completes")
        .expect("terminal repair inherits the cleanup-only revision");

    let repaired = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(repaired.session.status, SessionStatus::Terminated);
    assert_eq!(repaired.runtime_id, None);
    assert_eq!(repaired.rotation_claim, None);
    assert_eq!(repaired.pending_handover, None);
    assert_eq!(repaired.skill_packs, SkillPackState::Cleaned);
    assert_eq!(repaired.session.granted_skills, frozen_grant);
    assert_eq!(repaired.context_ref, residual.context_ref);
    assert_eq!(
        repaired.session.last_seq,
        residual.session.last_seq.saturating_add(1)
    );
    assert!(repaired.revision > cleaned.revision);
    assert_eq!(
        adapter.terminated.lock().await.len(),
        1,
        "the repair never repeats provider teardown"
    );
    let history_after_repair = sessions
        .events_since(&created.id, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|event| (event.seq, format!("{:?}", event.kind)))
        .collect::<Vec<_>>();
    assert_eq!(
        &history_after_repair[..history_before_repair.len()],
        history_before_repair.as_slice(),
        "terminal repair appends after the exact existing history"
    );
    assert_eq!(history_after_repair.len(), history_before_repair.len() + 1);
    assert!(history_after_repair.last().is_some_and(|(_, kind)| {
        kind.contains("StatusChanged") && kind.contains("Terminated")
    }));
}

#[tokio::test]
async fn live_terminal_runtime_repair_inherits_a_racing_empty_pack_cleanup_acknowledgement() {
    let sessions = Arc::new(FakeStore::default());
    let repair_store = Arc::new(CommitFaultStore::new(
        Arc::clone(&sessions),
        |record, events| {
            events.is_empty()
                && record.session.status == SessionStatus::Terminated
                && record.runtime_id.is_some()
                && record.rotation_claim.is_none()
                && record.pending_handover.is_none()
                && record.skill_packs != SkillPackState::Cleaned
        },
    ));
    let cleanup_store = Arc::new(CommitFaultStore::new(
        Arc::clone(&sessions),
        |record, events| {
            events.is_empty()
                && record.session.status == SessionStatus::Terminated
                && record.runtime_id.is_some()
                && record.skill_packs == SkillPackState::Cleaned
        },
    ));
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("live-terminal-runtime-cleanup-rebase-owner");
    let live = service_over(repair_store.clone(), Arc::clone(&adapter));
    let cleanup = service_over(cleanup_store.clone(), Arc::clone(&adapter));
    let created = live.create_session(&owner, spec()).await.expect("created");
    await_settled(&live, &owner, &created.id).await;
    live.flush(&created.id).await;

    let mut terminal = sessions.get(&created.id).await.unwrap().unwrap();
    let runtime = terminal
        .runtime_id
        .clone()
        .expect("the live entry retains its exact runtime");
    assert_eq!(terminal.skill_packs, SkillPackState::Staged(Vec::new()));
    let frozen_grant = terminal.session.granted_skills.clone();
    terminal.session.status = SessionStatus::Terminated;
    terminal.session.stop_reason = Some(crate::session::StopReason::UserTerminated);
    terminal.session.recovery_state = crate::session::RecoveryState::Retired;
    terminal.rotation_claim = None;
    terminal.pending_handover = None;
    terminal.context_ref = Some("context-live-terminal-runtime-cleanup-rebase".into());
    terminal.revision = sessions
        .commit_session_idempotent("test-live-terminal-retained-runtime", &terminal, &[])
        .await
        .expect("the terminal row retains its provider runtime");
    let history_before_repair = sessions
        .events_since(&created.id, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|event| (event.seq, format!("{:?}", event.kind)))
        .collect::<Vec<_>>();

    repair_store.pause_next();
    let repairing = tokio::spawn({
        let live = Arc::clone(&live);
        let owner = owner.clone();
        let id = created.id.clone();
        async move { live.terminate_session(&owner, &id).await }
    });
    repair_store.wait_until_held().await;

    cleanup_store.pause_next();
    let cleaning = tokio::spawn({
        let cleanup = Arc::clone(&cleanup);
        let id = created.id.clone();
        async move {
            cleanup
                .finish_terminal_skill_pack_cleanup_with_authority(
                    &id,
                    crate::coordination::SessionOperation::Reclaim,
                )
                .await
        }
    });
    cleanup_store.wait_until_held().await;
    assert_eq!(
        sessions.get(&created.id).await.unwrap().unwrap().revision,
        terminal.revision,
        "both writers observed the same terminal runtime boundary"
    );

    cleanup_store.release();
    let cleaned = cleaning
        .await
        .expect("the cleanup task completes")
        .expect("empty custody advances independently");
    assert_eq!(cleaned.skill_packs, SkillPackState::Cleaned);
    assert_eq!(cleaned.runtime_id.as_deref(), Some(runtime.as_str()));
    assert_eq!(cleaned.context_ref, terminal.context_ref);
    assert_eq!(cleaned.session.granted_skills, frozen_grant);
    assert_eq!(cleaned.session.last_seq, terminal.session.last_seq);
    assert!(cleaned.revision > terminal.revision);

    repair_store.release();
    repairing
        .await
        .expect("the live termination task completes")
        .expect("the retained runtime repair inherits cleanup-only custody");

    let repaired = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(repaired.session.status, SessionStatus::Terminated);
    assert_eq!(
        repaired.session.stop_reason,
        Some(crate::session::StopReason::UserTerminated)
    );
    assert_eq!(
        repaired.session.recovery_state,
        crate::session::RecoveryState::Retired
    );
    assert_eq!(repaired.runtime_id, None);
    assert_eq!(repaired.rotation_claim, None);
    assert_eq!(repaired.pending_handover, None);
    assert_eq!(repaired.skill_packs, SkillPackState::Cleaned);
    assert_eq!(repaired.context_ref, terminal.context_ref);
    assert_eq!(repaired.session.granted_skills, frozen_grant);
    assert_eq!(repaired.session.last_seq, terminal.session.last_seq);
    assert!(repaired.revision > cleaned.revision);
    assert!(!adapter.running.lock().await.contains_key(&runtime));
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        std::slice::from_ref(&runtime),
        "the exact retained runtime is torn down once"
    );
    let history_after_repair = sessions
        .events_since(&created.id, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|event| (event.seq, format!("{:?}", event.kind)))
        .collect::<Vec<_>>();
    assert_eq!(history_after_repair, history_before_repair);

    live.terminate_session(&owner, &created.id)
        .await
        .expect("the repaired terminal row remains idempotent");
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        &[runtime],
        "an idempotent retry does not repeat provider teardown"
    );
}

#[tokio::test]
async fn idempotent_live_terminal_repair_reclaims_a_stale_cross_replica_runtime() {
    let sessions = Arc::new(FakeStore::default());
    sessions.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("stale-cross-replica-runtime-owner");
    let stale = service_over(sessions.clone(), Arc::clone(&adapter));
    let created = stale.create_session(&owner, spec()).await.expect("created");
    await_settled(&stale, &owner, &created.id).await;
    stale.flush(&created.id).await;

    let mut rotated = sessions.get(&created.id).await.unwrap().unwrap();
    let h1 = rotated
        .runtime_id
        .clone()
        .expect("the stale replica retains H1");
    let h2 = format!("{h1}-cross-replica-h2");
    adapter
        .running
        .lock()
        .await
        .insert(h2.clone(), Some(created.id.0.clone()));
    rotated.runtime_id = Some(h2.clone());
    rotated.revision = sessions
        .commit_session_idempotent("test-cross-replica-runtime-h2", &rotated, &[])
        .await
        .expect("the replacement runtime is durable");

    let restarted = service_over(sessions.clone(), Arc::clone(&adapter));
    restarted
        .terminate_session(&owner, &created.id)
        .await
        .expect("the restarted replica terminates durable H2");
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let record = sessions.get(&created.id).await.unwrap().unwrap();
            if record.runtime_id.is_none() && record.skill_packs == SkillPackState::Cleaned {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("durable runtime and skill custody both settle");
    assert_eq!(terminal.session.status, SessionStatus::Terminated);
    assert!(adapter.running.lock().await.contains_key(&h1));
    assert!(!adapter.running.lock().await.contains_key(&h2));
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        std::slice::from_ref(&h2),
        "the restarted replica knows only durable H2"
    );
    {
        let entries = stale.entries.read().await;
        let entry = entries
            .get(&created.id)
            .expect("the other replica still retains its live entry");
        assert_eq!(entry.session.status, SessionStatus::Ready);
        assert_eq!(
            entry.runtime.as_ref().map(|runtime| runtime.0.as_str()),
            Some(h1.as_str())
        );
        assert!(entry.input.is_some());
    }
    let history_before_repair = sessions
        .events_since(&created.id, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|event| (event.seq, format!("{:?}", event.kind)))
        .collect::<Vec<_>>();

    stale
        .terminate_session(&owner, &created.id)
        .await
        .expect("idempotent termination reclaims stale local H1");

    let repaired = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(repaired.session.status, SessionStatus::Terminated);
    assert_eq!(repaired.runtime_id, None);
    assert_eq!(repaired.skill_packs, SkillPackState::Cleaned);
    assert!(!adapter.running.lock().await.contains_key(&h1));
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        &[h2, h1],
        "each replica tears down only the exact runtime it retained"
    );
    let entries = stale.entries.read().await;
    let entry = entries
        .get(&created.id)
        .expect("termination keeps the readable local entry");
    assert_eq!(entry.session.status, SessionStatus::Terminated);
    assert!(entry.runtime.is_none());
    assert!(entry.input.is_none());
    assert!(entry.conversation.is_none());
    drop(entries);
    let history_after_repair = sessions
        .events_since(&created.id, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|event| (event.seq, format!("{:?}", event.kind)))
        .collect::<Vec<_>>();
    assert_eq!(
        history_after_repair, history_before_repair,
        "the idempotent local-runtime repair appends no lifecycle event"
    );
}

#[tokio::test]
async fn queued_terminal_repairs_refuse_changed_exact_authority() {
    #[derive(Clone, Copy, Debug)]
    enum ChangedAuthority {
        Status,
        Runtime,
        RotationClaim,
        PendingHandover,
    }

    for changed in [
        ChangedAuthority::Status,
        ChangedAuthority::Runtime,
        ChangedAuthority::RotationClaim,
        ChangedAuthority::PendingHandover,
    ] {
        let sessions = Arc::new(FakeStore::default());
        let repair_store = Arc::new(CommitFaultStore::new(
            Arc::clone(&sessions),
            |record, events| {
                record.session.status == SessionStatus::Terminated
                    && record.runtime_id.is_some()
                    && record.rotation_claim.is_none()
                    && record.pending_handover.is_none()
                    && (events.is_empty()
                        || events.iter().any(|event| {
                            matches!(
                                event.kind,
                                EventKind::StatusChanged {
                                    status: SessionStatus::Terminated,
                                    ..
                                }
                            )
                        }))
            },
        ));
        let adapter = Arc::new(FakeAdapter::default());
        let owner = user(&format!("changed-terminal-authority-{changed:?}"));
        let service = service_over(repair_store.clone(), Arc::clone(&adapter));
        let created = service
            .create_session(&owner, spec())
            .await
            .expect("created");
        await_settled(&service, &owner, &created.id).await;
        service.flush(&created.id).await;

        let mut observed = sessions.get(&created.id).await.unwrap().unwrap();
        let observed_runtime = observed
            .runtime_id
            .clone()
            .expect("the terminal repair observes one runtime");
        if !matches!(changed, ChangedAuthority::Status) {
            observed.session.status = SessionStatus::Terminated;
            observed.session.stop_reason = Some(crate::session::StopReason::UserTerminated);
            observed.session.recovery_state = crate::session::RecoveryState::Retired;
            observed.rotation_claim = None;
            observed.pending_handover = None;
            observed.revision = sessions
                .commit_session_idempotent(
                    &format!("test-marked-repair-observed-{changed:?}"),
                    &observed,
                    &[],
                )
                .await
                .expect("the observed terminal boundary is durable");
        }
        let history_before = sessions
            .events_since(&created.id, 0)
            .await
            .unwrap()
            .into_iter()
            .map(|event| (event.seq, format!("{:?}", event.kind)))
            .collect::<Vec<_>>();

        repair_store.pause_next();
        let repairing = tokio::spawn({
            let service = Arc::clone(&service);
            let owner = owner.clone();
            let id = created.id.clone();
            async move { service.terminate_session(&owner, &id).await }
        });
        repair_store.wait_until_held().await;

        let mut rival = observed.clone();
        rival.context_ref = Some(format!("rival-context-{changed:?}"));
        match changed {
            ChangedAuthority::Status => rival.session.status = SessionStatus::Running,
            ChangedAuthority::Runtime => {
                rival.runtime_id = Some(format!("{observed_runtime}-successor"));
            }
            ChangedAuthority::RotationClaim => {
                rival.rotation_claim = Some(format!("rotn_successor_{changed:?}"));
            }
            ChangedAuthority::PendingHandover => {
                rival.pending_handover = Some(crate::store::PendingHandover {
                    claim: format!("rotn_successor_{changed:?}"),
                    snapshot_ref: format!("sessions/{}/successor.snapshot", created.id.0),
                    snapshot_at: "2026-08-27T05:06:07Z".into(),
                    rotations_from: rival.session.rotations,
                    rotations_to: rival.session.rotations.saturating_add(1),
                    target: crate::store::PendingHandoverTarget::RuntimeVersion {
                        from_runtime_version: rival.session.runtime_version.clone(),
                        runtime_version: "runtime:successor".into(),
                    },
                });
            }
        }
        rival.revision = sessions
            .commit_session_idempotent(
                &format!("test-marked-repair-rival-{changed:?}"),
                &rival,
                &[],
            )
            .await
            .expect("the rival authority advances while repair is queued");

        repair_store.release();
        let error = repairing
            .await
            .expect("the marked repair task completes")
            .expect_err("changed exact authority rejects the stale repair");
        assert!(
            matches!(
                &error,
                crate::error::CoreError::RuntimeUnavailable(message)
                    if message.contains("session revision changed")
            ),
            "{changed:?} returned {error:?}"
        );

        let durable = sessions.get(&created.id).await.unwrap().unwrap();
        assert_eq!(durable.revision, rival.revision, "{changed:?} revision");
        assert_eq!(
            durable.session.status, rival.session.status,
            "{changed:?} status"
        );
        assert_eq!(durable.runtime_id, rival.runtime_id, "{changed:?} runtime");
        assert_eq!(
            durable.rotation_claim, rival.rotation_claim,
            "{changed:?} rotation claim"
        );
        assert_eq!(
            durable.pending_handover, rival.pending_handover,
            "{changed:?} handover descriptor"
        );
        assert_eq!(
            durable.context_ref, rival.context_ref,
            "{changed:?} unrelated rival state"
        );
        assert_eq!(
            sessions
                .events_since(&created.id, 0)
                .await
                .unwrap()
                .into_iter()
                .map(|event| (event.seq, format!("{:?}", event.kind)))
                .collect::<Vec<_>>(),
            history_before,
            "{changed:?} appends no stale lifecycle event"
        );
        assert!(
            adapter.terminated.lock().await.is_empty(),
            "{changed:?} performs no provider teardown"
        );
        assert!(adapter.running.lock().await.contains_key(&observed_runtime));
    }
}

#[tokio::test]
async fn cleanup_outage_survives_restart_and_reconciliation() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("cleanup-restart-owner");
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    packs.fail_deletes.store(1, Ordering::SeqCst);

    first
        .terminate_session(&owner, &created.id)
        .await
        .expect("termination records cleanup debt");
    let pending = sessions
        .get(&created.id)
        .await
        .expect("terminal row is readable")
        .expect("terminal row remains the owner");
    assert_eq!(pending.skill_packs, SkillPackState::CleanupPending);
    assert!(packs.objects.lock().await.contains_key(&created.id.0));

    let replacement = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        adapter,
        "runtime:1",
        Arc::new(CountingSkills::refusing()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    replacement.reconcile().await;
    let cleaned = sessions
        .get(&created.id)
        .await
        .expect("cleaned row is readable")
        .expect("terminated history remains listed");
    assert_eq!(cleaned.skill_packs, SkillPackState::Cleaned);
    assert!(!packs.objects.lock().await.contains_key(&created.id.0));
    assert_eq!(packs.deletes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cleanup_acknowledgement_inherits_an_intervening_progress_commit() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("cleanup-ack-race-owner");
    let service = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        adapter,
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    packs.pause_next_delete();

    let ending = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move { service.terminate_session(&owner, &id).await }
    });
    packs.wait_until_delete_paused().await;
    let pending = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(pending.skill_packs, SkillPackState::CleanupPending);
    service
        .note_progress(&created.id, crate::session::ProvisioningStage::Ready)
        .await;
    let progressed = sessions.get(&created.id).await.unwrap().unwrap();
    assert!(progressed.revision > pending.revision);

    packs.resume_delete();
    ending
        .await
        .expect("termination task completes")
        .expect("termination succeeds");
    let cleaned = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(cleaned.skill_packs, SkillPackState::Cleaned);
    assert!(cleaned.revision > progressed.revision);
    let events = sessions.events_since(&created.id, 0).await.unwrap();
    assert!(events
        .iter()
        .any(|event| format!("{:?}", event.kind).contains("Progress")));
}

#[tokio::test]
async fn a_stale_queued_event_cannot_reopen_skill_custody_or_grant() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("stale-cleanup-event-owner");
    let service = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    let authoritative_grant = sessions
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .session
        .granted_skills;

    let stale = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::refusing()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    assert_eq!(stale.reconcile().await.readopted_sessions, 1);
    packs.pause_next_delete();
    let ending = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move { service.terminate_session(&owner, &id).await }
    });
    packs.wait_until_delete_paused().await;
    assert_eq!(
        sessions
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .skill_packs,
        SkillPackState::CleanupPending
    );

    let obsolete_grant = Some(vec![crate::environment::GrantedSkill {
        id: SkillId::new("obsolete-stale-skill"),
        version: Some("0.1".into()),
    }]);
    assert_ne!(obsolete_grant, authoritative_grant);
    let lane = stale.event_logs.lane(&created.id);
    {
        let mut entries = stale.entries.write().await;
        let entry = entries
            .get_mut(&created.id)
            .expect("the second replica still has its stale Ready entry");
        entry.session.granted_skills.clone_from(&obsolete_grant);
        crate::log::append(entry, crate::event::EventKind::TerminalDetached);
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stale event settles before cleanup acknowledgement");
    let pending = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(pending.skill_packs, SkillPackState::CleanupPending);
    assert_eq!(pending.session.granted_skills, authoritative_grant);

    packs.resume_delete();
    ending
        .await
        .expect("termination task completes")
        .expect("termination succeeds");
    let cleaned = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(cleaned.skill_packs, SkillPackState::Cleaned);
    assert_eq!(cleaned.session.granted_skills, authoritative_grant);
    assert!(sessions
        .events_since(&created.id, 0)
        .await
        .unwrap()
        .iter()
        .any(|event| matches!(
            event.kind,
            crate::event::StoredEventKind::Known(crate::event::EventKind::TerminalDetached)
        )));
}

#[tokio::test]
async fn a_missing_cleanup_port_preserves_debt_for_a_configured_restart() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("missing-cleanup-port-owner");
    let first = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;

    let missing_port = service_on_version_with_skills(
        Arc::clone(&sessions),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(CountingSkills::refusing()),
    );
    missing_port
        .terminate_session(&owner, &created.id)
        .await
        .expect("terminal state remains commit-able without the object port");
    missing_port.reconcile().await;
    let pending = sessions.get(&created.id).await.unwrap().unwrap();
    assert_eq!(pending.skill_packs, SkillPackState::CleanupPending);
    assert!(packs.objects.lock().await.contains_key(&created.id.0));
    assert_eq!(packs.deletes.load(Ordering::SeqCst), 0);

    let configured = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        adapter,
        "runtime:1",
        Arc::new(CountingSkills::refusing()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    configured.reconcile().await;
    assert_eq!(
        sessions
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .skill_packs,
        SkillPackState::Cleaned
    );
    assert!(!packs.objects.lock().await.contains_key(&created.id.0));
    assert_eq!(packs.deletes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn delete_keeps_the_owner_row_while_skill_cleanup_fails() {
    let sessions = Arc::new(FakeStore::default());
    let packs = Arc::new(MemorySkillPacks::default());
    let owner = user("cleanup-delete-owner");
    let service = service_on_version_with_skill_store(
        Arc::clone(&sessions),
        Arc::new(FakeAdapter::default()),
        "runtime:1",
        Arc::new(CountingSkills::working()),
        Arc::clone(&packs) as Arc<dyn SkillPackStore>,
    );
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    packs.fail_deletes.store(1, Ordering::SeqCst);
    service
        .terminate_session(&owner, &created.id)
        .await
        .expect("termination records cleanup debt");
    let events_before = sessions
        .events
        .lock()
        .await
        .iter()
        .filter(|event| event.session_id == created.id)
        .count();

    packs.fail_deletes.store(2, Ordering::SeqCst);
    assert!(service.delete_session(&owner, &created.id).await.is_err());
    let retained = sessions
        .get(&created.id)
        .await
        .expect("retained row is readable")
        .expect("cleanup failure preserves its owner row");
    assert_eq!(retained.skill_packs, SkillPackState::CleanupPending);
    assert_eq!(
        sessions
            .events
            .lock()
            .await
            .iter()
            .filter(|event| event.session_id == created.id)
            .count(),
        events_before
    );

    service
        .delete_session(&owner, &created.id)
        .await
        .expect("retry cleans custody before deleting the row");
    assert!(sessions
        .get(&created.id)
        .await
        .expect("store reads")
        .is_none());
}
