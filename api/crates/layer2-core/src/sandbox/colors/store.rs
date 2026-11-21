use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::{SandboxId, SubjectId};

use super::super::store::SandboxStoreError;
use super::SandboxColor;

/// Every colour recorded for one owner's Sandboxes. Revision 0 means the owner
/// has no palette yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnerSandboxColors {
    pub revision: u64,
    pub colors: BTreeMap<SandboxId, SandboxColor>,
}

/// One palette per owner, so distinctness is checked and recorded atomically
/// against everything that owner already wears.
#[async_trait]
pub trait SandboxColorStore: Send + Sync {
    async fn owner_colors(
        &self,
        owner: &SubjectId,
    ) -> Result<OwnerSandboxColors, SandboxStoreError>;

    /// Adds `assignments` only while the owner's palette is still at
    /// `expected_revision`, returning `Conflict` otherwise. A recorded colour
    /// is never replaced, so a Sandbox keeps the colour it was first shown in.
    async fn record(
        &self,
        owner: &SubjectId,
        expected_revision: u64,
        assignments: &[(SandboxId, SandboxColor)],
    ) -> Result<(), SandboxStoreError>;
}

/// Process-local palettes for tests and storage-free composition.
#[derive(Default)]
pub struct InMemorySandboxColorStore {
    palettes: Mutex<HashMap<SubjectId, OwnerSandboxColors>>,
}

impl InMemorySandboxColorStore {
    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<SubjectId, OwnerSandboxColors>>, SandboxStoreError>
    {
        self.palettes.lock().map_err(|_| {
            SandboxStoreError::Unavailable("sandbox colour palette lock poisoned".into())
        })
    }
}

#[async_trait]
impl SandboxColorStore for InMemorySandboxColorStore {
    async fn owner_colors(
        &self,
        owner: &SubjectId,
    ) -> Result<OwnerSandboxColors, SandboxStoreError> {
        Ok(self.lock()?.get(owner).cloned().unwrap_or_default())
    }

    async fn record(
        &self,
        owner: &SubjectId,
        expected_revision: u64,
        assignments: &[(SandboxId, SandboxColor)],
    ) -> Result<(), SandboxStoreError> {
        let mut palettes = self.lock()?;
        let palette = palettes.entry(owner.clone()).or_default();
        if palette.revision != expected_revision
            || assignments
                .iter()
                .any(|(sandbox, _)| palette.colors.contains_key(sandbox))
        {
            return Err(SandboxStoreError::Conflict);
        }
        palette.colors.extend(assignments.iter().cloned());
        palette.revision += 1;
        Ok(())
    }
}
