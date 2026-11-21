use std::time::Instant;

use super::{EpisodeTransition, Phase};

pub(super) fn abandon_episode(phase: &mut Phase, at: Instant) -> Option<EpisodeTransition> {
    let previous = std::mem::take(phase);
    let Phase::Open {
        opened_at,
        limit,
        peak_permille,
        peak,
        ..
    } = previous
    else {
        return None;
    };
    Some(EpisodeTransition::Closed {
        peak,
        limit,
        peak_permille,
        duration_seconds: at.duration_since(opened_at).as_secs() as u32,
    })
}
