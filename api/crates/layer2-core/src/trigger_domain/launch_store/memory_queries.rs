use std::collections::BTreeMap;

use crate::ids::{PlacementId, SubjectId, TriggerId};

use super::{memory::State, LaunchPage, LaunchPageRequest, LaunchPlacementStats, LaunchStoreError};

pub(super) fn list(
    state: &State,
    owner: &SubjectId,
    page: &LaunchPageRequest,
) -> Result<LaunchPage, LaunchStoreError> {
    page.validate()?;
    let boundary = page.cursor_boundary()?;
    let mut launches = state
        .launches
        .values()
        .filter(|launch| &launch.owner == owner)
        .filter(|launch| {
            page.trigger
                .as_ref()
                .is_none_or(|trigger| trigger == &launch.trigger_id)
        })
        .filter(|launch| page.state.is_none_or(|state| state == launch.state))
        .filter(|launch| {
            boundary.as_ref().is_none_or(|boundary| {
                (&launch.created_at, &launch.id) < (&boundary.created_at, &boundary.id)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    launches
        .sort_by(|left, right| (&right.created_at, &right.id).cmp(&(&left.created_at, &left.id)));
    let has_more = launches.len() > usize::from(page.limit);
    launches.truncate(usize::from(page.limit));
    let next = if has_more {
        launches
            .last()
            .map(|launch| page.cursor_after(launch))
            .transpose()?
    } else {
        None
    };
    Ok(LaunchPage {
        items: launches,
        next,
    })
}

pub(super) fn placement_stats(
    state: &State,
    trigger_id: &TriggerId,
) -> Result<Vec<LaunchPlacementStats>, LaunchStoreError> {
    let mut stats = BTreeMap::<Option<PlacementId>, (u64, u64)>::new();
    for launch in state
        .launches
        .values()
        .filter(|launch| &launch.trigger_id == trigger_id)
    {
        let entry = stats.entry(launch.placement_id.clone()).or_default();
        entry.0 = entry
            .0
            .checked_add(1)
            .ok_or_else(|| corrupt("placement Launch count overflow"))?;
        if launch.ready_at.is_some() {
            entry.1 = entry
                .1
                .checked_add(1)
                .ok_or_else(|| corrupt("placement ready-Agent count overflow"))?;
        }
    }
    Ok(stats
        .into_iter()
        .map(
            |(placement_id, (launches_total, agents_ready_total))| LaunchPlacementStats {
                placement_id,
                launches_total,
                agents_ready_total,
            },
        )
        .collect())
}

fn corrupt(detail: &str) -> LaunchStoreError {
    LaunchStoreError::Corrupt(detail.into())
}
