use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::profiles::{AgentProvider, ModelAccess, SandboxProfileContent, SandboxProfileSnapshot};
use crate::sandbox::store::{SandboxStore, SandboxStoreError};
use crate::sandbox::{
    InMemorySandboxStore, Sandbox, SandboxCredentialState, SandboxLimits, SandboxOrigin,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
};
use crate::{SandboxId, SandboxProfileId, SubjectId};

use super::palette::{admissible, choose, spacing};
use super::*;

/// A replayable source: xorshift64, so a failing choice can be reproduced.
struct Seeded(Mutex<u64>);

impl Seeded {
    fn new(seed: u64) -> Self {
        Self(Mutex::new(seed))
    }
}

impl ColorRandom for Seeded {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), PaletteError> {
        let mut state = self.0.lock().unwrap();
        for byte in bytes {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            *byte = (*state >> 24) as u8;
        }
        Ok(())
    }
}

struct Constant(u8);

impl ColorRandom for Constant {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), PaletteError> {
        bytes.fill(self.0);
        Ok(())
    }
}

struct Broken;

impl ColorRandom for Broken {
    fn fill(&self, _: &mut [u8]) -> Result<(), PaletteError> {
        Err(PaletteError::RandomUnavailable)
    }
}

/// Answers `Conflict` to the first `conflicts` writes, as if another reader
/// recorded first, then behaves like the in-memory store.
struct Contended {
    inner: InMemorySandboxColorStore,
    conflicts: AtomicUsize,
}

#[async_trait]
impl SandboxColorStore for Contended {
    async fn owner_colors(
        &self,
        owner: &SubjectId,
    ) -> Result<OwnerSandboxColors, SandboxStoreError> {
        self.inner.owner_colors(owner).await
    }

    async fn record(
        &self,
        owner: &SubjectId,
        expected_revision: u64,
        assignments: &[(SandboxId, SandboxColor)],
    ) -> Result<(), SandboxStoreError> {
        if self
            .conflicts
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(SandboxStoreError::Conflict);
        }
        self.inner
            .record(owner, expected_revision, assignments)
            .await
    }
}

struct Unavailable;

#[async_trait]
impl SandboxColorStore for Unavailable {
    async fn owner_colors(&self, _: &SubjectId) -> Result<OwnerSandboxColors, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable("palette offline".into()))
    }

    async fn record(
        &self,
        _: &SubjectId,
        _: u64,
        _: &[(SandboxId, SandboxColor)],
    ) -> Result<(), SandboxStoreError> {
        Err(SandboxStoreError::Unavailable("palette offline".into()))
    }
}

fn sandbox(id: &str, owner: &str, created_at: &str, status: SandboxStatus) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId::new(owner),
        name: format!("sandbox-{id}"),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_colors".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, created_at.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: created_at.into(),
        created_at: created_at.into(),
        updated_at: created_at.into(),
        retired_at: (status == SandboxStatus::Retired).then(|| created_at.into()),
        purge_after: None,
        revision: 1,
    }
}

async fn stored(sandboxes: &[&Sandbox]) -> Arc<InMemorySandboxStore> {
    let store = Arc::new(InMemorySandboxStore::default());
    for sandbox in sandboxes {
        store.create_sandbox((*sandbox).clone()).await.unwrap();
    }
    store
}

fn service(
    colors: Arc<dyn SandboxColorStore>,
    sandboxes: Arc<InMemorySandboxStore>,
    seed: u64,
) -> Arc<SandboxColorService> {
    SandboxColorService::new(colors, sandboxes, Arc::new(Seeded::new(seed)))
}

#[test]
fn colour_has_exactly_one_spelling() {
    let color = SandboxColor::parse("#0a7fe3").unwrap();
    assert_eq!(color.rgb(), [0x0a, 0x7f, 0xe3]);
    assert_eq!(color.to_string(), "#0a7fe3");
    for invalid in [
        "0a7fe3", "#0A7FE3", "#0a7fe", "#0a7fe3f", "#0a7fg3", "", "#",
    ] {
        assert_eq!(
            SandboxColor::parse(invalid),
            Err(InvalidSandboxColor),
            "{invalid}"
        );
    }
    assert_eq!(
        serde_json::to_string(&color).unwrap(),
        "\"#0a7fe3\"".to_string()
    );
    assert_eq!(
        serde_json::from_str::<SandboxColor>("\"#0a7fe3\"").unwrap(),
        color
    );
    assert!(serde_json::from_str::<SandboxColor>("\"#0A7FE3\"").is_err());
}

#[test]
fn pool_excludes_status_colours_and_illegible_bodies() {
    for reserved in ["#ffc45a", "#bc3b26", "#ffb84d", "#c4402a"] {
        assert!(
            !admissible(SandboxColor::parse(reserved).unwrap()),
            "{reserved}"
        );
    }
    for illegible in ["#101010", "#fafafa", "#a0a0a0", "#1b2a8f"] {
        assert!(
            !admissible(SandboxColor::parse(illegible).unwrap()),
            "{illegible}"
        );
    }
    for vivid in ["#2f96fa", "#62d079", "#ec29e5", "#46d2f9"] {
        assert!(admissible(SandboxColor::parse(vivid).unwrap()), "{vivid}");
    }
}

#[test]
fn choice_is_admissible_replayable_and_keeps_clear_of_neighbours() {
    let occupied = [SandboxColor::parse("#2f96fa").unwrap()];
    let first = choose(&occupied, &Seeded::new(11)).unwrap();
    assert!(admissible(first));
    assert_eq!(choose(&occupied, &Seeded::new(11)).unwrap(), first);
    assert!(spacing(first, &occupied) > 0.25);

    let mut taken = Vec::new();
    let random = Seeded::new(5);
    for _ in 0..10 {
        let next = choose(&taken, &random).unwrap();
        assert!(!taken.contains(&next));
        assert!(spacing(next, &taken) >= 0.10, "{next} crowds {taken:?}");
        taken.push(next);
    }
}

#[test]
fn choice_reports_a_broken_or_degenerate_source() {
    assert_eq!(choose(&[], &Broken), Err(PaletteError::RandomUnavailable));
    // Every byte zero is black, which is never admissible.
    assert_eq!(choose(&[], &Constant(0)), Err(PaletteError::Exhausted));
}

#[tokio::test]
async fn memory_store_refuses_stale_revisions_and_recolouring() {
    let store = InMemorySandboxColorStore::default();
    let owner = SubjectId::new("owner");
    let id = SandboxId("sbx_one".into());
    let blue = SandboxColor::parse("#2f96fa").unwrap();
    store
        .record(&owner, 0, &[(id.clone(), blue)])
        .await
        .unwrap();
    assert_eq!(
        store
            .record(&owner, 0, &[(SandboxId("sbx_two".into()), blue)])
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let pink = SandboxColor::parse("#ec29e5").unwrap();
    assert_eq!(
        store.record(&owner, 1, &[(id.clone(), pink)]).await,
        Err(SandboxStoreError::Conflict)
    );
    let palette = store.owner_colors(&owner).await.unwrap();
    assert_eq!(palette.revision, 1);
    assert_eq!(palette.colors.get(&id), Some(&blue));
    assert_eq!(
        store.owner_colors(&SubjectId::new("other")).await.unwrap(),
        OwnerSandboxColors::default()
    );
}

#[tokio::test]
async fn owner_sandboxes_get_distinct_stable_colours_oldest_first() {
    let older = sandbox(
        "sbx_b",
        "owner",
        "2026-11-01T00:00:00Z",
        SandboxStatus::Active,
    );
    let newer = sandbox(
        "sbx_a",
        "owner",
        "2026-11-01T00:05:00Z",
        SandboxStatus::Paused,
    );
    let palettes = Arc::new(InMemorySandboxColorStore::default());
    let colors = service(palettes.clone(), stored(&[&older, &newer]).await, 3);
    let owner = SubjectId::new("owner");

    let first = colors.colors(&owner, &[&newer, &older]).await;
    assert_eq!(first.len(), 2);
    assert_ne!(first[&older.id], first[&newer.id]);
    // The older Sandbox was chosen first, against an empty palette.
    assert_eq!(first[&older.id], choose(&[], &Seeded::new(3)).unwrap());

    let again = colors.colors(&owner, &[&older, &newer]).await;
    assert_eq!(again, first);
    assert_eq!(palettes.owner_colors(&owner).await.unwrap().revision, 1);
}

#[tokio::test]
async fn new_colour_keeps_clear_of_unlisted_live_sandboxes_only() {
    let kept = sandbox(
        "sbx_kept",
        "owner",
        "2026-11-01T00:00:00Z",
        SandboxStatus::Active,
    );
    let gone = sandbox(
        "sbx_gone",
        "owner",
        "2026-11-01T00:01:00Z",
        SandboxStatus::Retired,
    );
    let fresh = sandbox(
        "sbx_fresh",
        "owner",
        "2026-11-01T00:02:00Z",
        SandboxStatus::Provisioning,
    );
    let owner = SubjectId::new("owner");
    let palettes = Arc::new(InMemorySandboxColorStore::default());
    let kept_color = SandboxColor::parse("#2f96fa").unwrap();
    // The retired Sandbox holds exactly the colour the fresh one would pick.
    // Were it still occupied, any other candidate would beat a distance of 0.
    let freed = choose(&[kept_color], &Seeded::new(17)).unwrap();
    palettes
        .record(
            &owner,
            0,
            &[(kept.id.clone(), kept_color), (gone.id.clone(), freed)],
        )
        .await
        .unwrap();
    let colors = service(palettes, stored(&[&kept, &gone, &fresh]).await, 17);

    let painted = colors.colors(&owner, &[&fresh]).await;
    assert_eq!(painted.get(&fresh.id), Some(&freed));
}

#[tokio::test]
async fn colour_scan_follows_empty_filtered_pages_to_later_live_sandboxes() {
    let store = Arc::new(InMemorySandboxStore::default());
    let owner = SubjectId::new("owner");
    for index in 0..crate::sandbox::store::MAX_PAGE_SIZE {
        store
            .create_sandbox(sandbox(
                &format!("sbx_a{index:04}"),
                "owner",
                "2026-11-01T00:00:00Z",
                SandboxStatus::Retired,
            ))
            .await
            .unwrap();
    }
    let kept = sandbox(
        "sbx_y",
        "owner",
        "2026-11-01T00:01:00Z",
        SandboxStatus::Active,
    );
    let fresh = sandbox(
        "sbx_z",
        "owner",
        "2026-11-01T00:02:00Z",
        SandboxStatus::Provisioning,
    );
    store.create_sandbox(kept.clone()).await.unwrap();
    store.create_sandbox(fresh.clone()).await.unwrap();
    let first = store
        .list_live_sandboxes(&owner, &crate::sandbox::store::PageRequest::default())
        .await
        .unwrap();
    assert!(first.items.is_empty());
    assert!(first.next.is_some());
    let palettes = Arc::new(InMemorySandboxColorStore::default());
    let occupied = choose(&[], &Seeded::new(17)).unwrap();
    palettes
        .record(&owner, 0, &[(kept.id, occupied)])
        .await
        .unwrap();
    let expected = choose(&[occupied], &Seeded::new(17)).unwrap();
    assert_ne!(expected, occupied);
    assert_eq!(
        service(palettes, store, 17)
            .colors(&owner, &[&fresh])
            .await
            .get(&fresh.id),
        Some(&expected)
    );
}

#[tokio::test]
async fn retiring_sandboxes_are_never_given_a_colour() {
    let retired = sandbox(
        "sbx_retired",
        "owner",
        "2026-11-01T00:00:00Z",
        SandboxStatus::Retired,
    );
    let palettes = Arc::new(InMemorySandboxColorStore::default());
    let colors = service(palettes.clone(), stored(&[&retired]).await, 9);
    let owner = SubjectId::new("owner");
    assert!(colors.colors(&owner, &[&retired]).await.is_empty());
    assert_eq!(palettes.owner_colors(&owner).await.unwrap().revision, 0);
}

#[tokio::test]
async fn another_owners_palette_is_read_but_never_written() {
    let painted = sandbox(
        "sbx_painted",
        "other",
        "2026-11-01T00:00:00Z",
        SandboxStatus::Active,
    );
    let bare = sandbox(
        "sbx_bare",
        "other",
        "2026-11-01T00:01:00Z",
        SandboxStatus::Active,
    );
    let other = SubjectId::new("other");
    let palettes = Arc::new(InMemorySandboxColorStore::default());
    let color = SandboxColor::parse("#62d079").unwrap();
    palettes
        .record(&other, 0, &[(painted.id.clone(), color)])
        .await
        .unwrap();
    let colors = service(palettes.clone(), stored(&[&painted, &bare]).await, 1);

    let seen = colors
        .colors(&SubjectId::new("viewer"), &[&painted, &bare])
        .await;
    assert_eq!(seen.get(&painted.id), Some(&color));
    assert_eq!(seen.get(&bare.id), None);
    assert_eq!(palettes.owner_colors(&other).await.unwrap().revision, 1);
}

#[tokio::test]
async fn a_lost_race_retries_and_endless_contention_shows_no_colour() {
    let sandbox = sandbox(
        "sbx_race",
        "owner",
        "2026-11-01T00:00:00Z",
        SandboxStatus::Active,
    );
    let owner = SubjectId::new("owner");
    let briefly = Arc::new(Contended {
        inner: InMemorySandboxColorStore::default(),
        conflicts: AtomicUsize::new(2),
    });
    let colors = service(briefly, stored(&[&sandbox]).await, 2);
    assert_eq!(colors.colors(&owner, &[&sandbox]).await.len(), 1);

    let endlessly = Arc::new(Contended {
        inner: InMemorySandboxColorStore::default(),
        conflicts: AtomicUsize::new(usize::MAX),
    });
    let colors = service(endlessly, stored(&[&sandbox]).await, 2);
    assert!(colors.colors(&owner, &[&sandbox]).await.is_empty());
}

#[tokio::test]
async fn unavailable_storage_or_randomness_leaves_sandboxes_uncoloured() {
    let sandbox = sandbox(
        "sbx_dark",
        "owner",
        "2026-11-01T00:00:00Z",
        SandboxStatus::Active,
    );
    let owner = SubjectId::new("owner");
    let colors = service(Arc::new(Unavailable), stored(&[&sandbox]).await, 4);
    assert!(colors.colors(&owner, &[&sandbox]).await.is_empty());

    let colors = SandboxColorService::new(
        Arc::new(InMemorySandboxColorStore::default()),
        stored(&[&sandbox]).await,
        Arc::new(Broken),
    );
    assert!(colors.colors(&owner, &[&sandbox]).await.is_empty());
}
