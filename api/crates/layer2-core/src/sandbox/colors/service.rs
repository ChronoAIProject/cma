use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{SandboxId, SubjectId};

use super::super::store::{PageRequest, SandboxStore, SandboxStoreError};
use super::super::{Sandbox, SandboxStatus};
use super::palette::{self, ColorRandom, PaletteError};
use super::store::{OwnerSandboxColors, SandboxColorStore};
use super::SandboxColor;

/// Concurrent first reads of one owner's new Sandboxes are rare; a few
/// attempts settle them, and a reader that still loses shows no colour once.
const RECORD_ATTEMPTS: usize = 3;

#[derive(Debug, thiserror::Error)]
enum AssignError {
    #[error(transparent)]
    Store(#[from] SandboxStoreError),
    #[error(transparent)]
    Palette(#[from] PaletteError),
    #[error("the owner's palette kept changing")]
    Contended,
}

/// Gives every Sandbox one colour, shared by all of its Agents, and keeps the
/// colours of one owner's live Sandboxes as far apart as the pool allows.
pub struct SandboxColorService {
    colors: Arc<dyn SandboxColorStore>,
    sandboxes: Arc<dyn SandboxStore>,
    random: Arc<dyn ColorRandom>,
}

impl SandboxColorService {
    pub fn new(
        colors: Arc<dyn SandboxColorStore>,
        sandboxes: Arc<dyn SandboxStore>,
        random: Arc<dyn ColorRandom>,
    ) -> Arc<Self> {
        Arc::new(Self {
            colors,
            sandboxes,
            random,
        })
    }

    /// The colour of each of `sandboxes` that has one. The caller's own
    /// Sandboxes without a colour are given one now, oldest first; another
    /// owner's palette is only read, never written on that owner's behalf.
    ///
    /// A colour is presentation, so failing to read or record one leaves those
    /// Sandboxes uncoloured instead of failing the read that asked.
    pub async fn colors(
        &self,
        caller: &SubjectId,
        sandboxes: &[&Sandbox],
    ) -> BTreeMap<SandboxId, SandboxColor> {
        let owners: BTreeSet<&SubjectId> = sandboxes.iter().map(|sandbox| &sandbox.owner).collect();
        let mut result = BTreeMap::new();
        for owner in owners {
            let owned: Vec<&Sandbox> = sandboxes
                .iter()
                .copied()
                .filter(|sandbox| &sandbox.owner == owner)
                .collect();
            let palette = if owner == caller {
                self.assign_missing(owner, &owned).await
            } else {
                self.colors
                    .owner_colors(owner)
                    .await
                    .map_err(AssignError::from)
            };
            match palette {
                Ok(palette) => result.extend(owned.iter().filter_map(|sandbox| {
                    palette
                        .colors
                        .get(&sandbox.id)
                        .map(|color| (sandbox.id.clone(), *color))
                })),
                Err(error) => tracing::warn!(
                    sandboxes = owned.len(),
                    %error,
                    "sandbox colours unavailable; showing these sandboxes uncoloured"
                ),
            }
        }
        result
    }

    async fn assign_missing(
        &self,
        owner: &SubjectId,
        listed: &[&Sandbox],
    ) -> Result<OwnerSandboxColors, AssignError> {
        for _ in 0..RECORD_ATTEMPTS {
            let mut palette = self.colors.owner_colors(owner).await?;
            let mut missing: Vec<&Sandbox> = listed
                .iter()
                .copied()
                .filter(|sandbox| wears_color(sandbox.status))
                .filter(|sandbox| !palette.colors.contains_key(&sandbox.id))
                .collect();
            if missing.is_empty() {
                return Ok(palette);
            }
            missing.sort_by(|left, right| {
                (&left.created_at, &left.id).cmp(&(&right.created_at, &right.id))
            });
            let live = self.live_sandboxes(owner).await?;
            let mut occupied: Vec<SandboxColor> = palette
                .colors
                .iter()
                .filter(|(sandbox, _)| live.contains(*sandbox))
                .map(|(_, color)| *color)
                .collect();
            let mut assignments = Vec::with_capacity(missing.len());
            for sandbox in missing {
                let color = palette::choose(&occupied, self.random.as_ref())?;
                tracing::info!(
                    sandbox_id = %sandbox.id,
                    %color,
                    spacing = palette::spacing(color, &occupied),
                    "assigned sandbox colour"
                );
                occupied.push(color);
                assignments.push((sandbox.id.clone(), color));
            }
            match self
                .colors
                .record(owner, palette.revision, &assignments)
                .await
            {
                Ok(()) => {
                    palette.revision += 1;
                    palette.colors.extend(assignments);
                    return Ok(palette);
                }
                Err(SandboxStoreError::Conflict) => {
                    tracing::debug!("sandbox colour palette changed while assigning; retrying");
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(AssignError::Contended)
    }

    /// Ids of the owner's Sandboxes that still wear their colour, across every
    /// page, so a new colour keeps clear of Sandboxes the caller did not list.
    async fn live_sandboxes(
        &self,
        owner: &SubjectId,
    ) -> Result<BTreeSet<SandboxId>, SandboxStoreError> {
        let mut live = BTreeSet::new();
        let mut cursor = None;
        loop {
            let page = self
                .sandboxes
                .list_sandboxes(
                    owner,
                    &PageRequest {
                        cursor,
                        ..PageRequest::default()
                    },
                )
                .await?;
            live.extend(
                page.items
                    .into_iter()
                    .filter(|sandbox| wears_color(sandbox.status))
                    .map(|sandbox| sandbox.id),
            );
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(live),
            }
        }
    }
}

/// A retiring or retired Sandbox is leaving the owner's view, so its colour
/// is free for the next Sandbox and it is never given a new one.
fn wears_color(status: SandboxStatus) -> bool {
    !matches!(status, SandboxStatus::Retiring | SandboxStatus::Retired)
}
