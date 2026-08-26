//! Durable skill-pack replay and lifecycle cleanup.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::store::{
    SessionStore, SkillPack, SkillPackState, SkillPackStore, SkillSource, StoreError,
};
use crate::test_support::{
    await_settled, service_on_version, service_on_version_with_skill_store, spec, user,
    FakeAdapter, FakeStore,
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
        self.objects.lock().await.remove(&id.0);
        Ok(())
    }
}

struct CountingSkills {
    calls: AtomicUsize,
    refuses: bool,
    requests: Mutex<Vec<(bool, Vec<SkillId>)>>,
}

impl CountingSkills {
    fn working() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            refuses: false,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn refusing() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            refuses: true,
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
                version: Some("7.3".into()),
                files: vec![
                    ("SKILL.md".into(), b"# Durable fixture".to_vec()),
                    ("references/checks.txt".into(), b"one\ntwo\n".to_vec()),
                ],
            })
            .collect())
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
}

#[tokio::test]
async fn a_missing_legacy_pack_recovers_only_the_frozen_grant() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("legacy-skill-owner");
    let old = service_on_version(Arc::clone(&sessions), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&owner, spec()).await.expect("created");
    await_settled(&old, &owner, &created.id).await;

    let mut legacy = sessions
        .get(&created.id)
        .await
        .expect("legacy session is readable")
        .expect("legacy session exists");
    legacy.skill_packs = SkillPackState::Legacy;
    legacy.session.granted_skills = Some(vec![crate::environment::GrantedSkill {
        id: SkillId::new("frozen-private-skill"),
        version: Some("7.3".into()),
    }]);
    sessions
        .seed(&legacy)
        .await
        .expect("the legacy representation is installed");

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
    assert_eq!(recovered.skill_packs, SkillPackState::Stored);
    assert_eq!(
        recovered.session.granted_skills,
        legacy.session.granted_skills
    );
}

#[tokio::test]
async fn a_legacy_session_without_a_frozen_grant_never_consults_the_profile() {
    let sessions = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("unknown-legacy-skill-owner");
    let old = service_on_version(Arc::clone(&sessions), Arc::clone(&adapter), "runtime:1");
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
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    assert!(packs.objects.lock().await.is_empty());
    let deferred = sessions
        .get(&created.id)
        .await
        .expect("deferred session is readable")
        .expect("deferred session exists");
    assert_eq!(deferred.skill_packs, SkillPackState::Legacy);
    assert_eq!(
        deferred.runtime_id.as_deref(),
        Some(original_runtime.as_str())
    );
    assert!(adapter.terminated.lock().await.is_empty());
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
        2,
        "both the terminal transition and a later terminal observation may clean up"
    );
}
