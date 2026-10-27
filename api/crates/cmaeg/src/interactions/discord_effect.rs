//! Retained capability and delete-vs-callback admission on the original card CAS.
use super::{discord_authority::Candidate, Card, Error, InteractionStore};
use crate::delivery::intents::DeliveryOutcome;
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) deletion: Option<Deletion>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Deletion {
    claim: String,
    outcome: Outcome,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Pending,
    Confirmed,
    Unknown,
}

pub(super) fn deleting(card: &Card) -> bool {
    card.discord_effect
        .as_ref()
        .is_some_and(|state| state.deletion.is_some())
}
pub(super) fn valid(card: &Card) -> bool {
    card.discord_effect.as_ref().is_none_or(|state| {
        card.discord.is_some()
            && state.deletion.as_ref().is_none_or(|delete| {
                card.superseded
                    && card.discord_edit.is_none()
                    && uuid::Uuid::parse_str(&delete.claim).is_ok()
            })
    })
}
impl InteractionStore {
    /// Caller holds the shared destination lock. A concurrent click may win the
    /// CAS; callers then reload its immutable result before attempting admission.
    pub(crate) async fn begin_discord_effect(
        &self,
        mut candidate: Candidate,
        claim: Option<&str>,
        adoption_enabled: bool,
    ) -> Result<(), Error> {
        let card = &mut candidate.card;
        if card.superseded || (card.discord_effect.is_none() && !adoption_enabled) {
            return Err(Error::Denied);
        }
        if card.discord_effect.is_some() && claim.is_none() {
            return Ok(());
        }
        let state = card.discord_effect.get_or_insert_with(State::default);
        if let Some(claim) = claim {
            uuid::Uuid::parse_str(claim).map_err(|_| Error::Denied)?;
            state.deletion = Some(Deletion {
                claim: claim.into(),
                outcome: Outcome::Pending,
            });
            card.superseded = true;
        }
        self.save(card, Some(candidate.version)).await?;
        Ok(())
    }
    pub(crate) async fn finish_discord_delete(
        &self,
        id: &str,
        claim: &str,
        outcome: &DeliveryOutcome,
    ) -> Result<(), Error> {
        let (mut card, version) = self.load(id).await?;
        let state = card.discord_effect.as_mut().ok_or(Error::Denied)?;
        let deletion = state.deletion.as_mut().ok_or(Error::Denied)?;
        if deletion.claim != claim || !matches!(deletion.outcome, Outcome::Pending) {
            return Err(Error::Denied);
        }
        match outcome {
            DeliveryOutcome::Confirmed { .. } => deletion.outcome = Outcome::Confirmed,
            DeliveryOutcome::Rejected { .. } | DeliveryOutcome::RateLimited { .. } => {
                // Only a definite no-write result may restore controls. The CAS
                // preserves a click that won before delete admission.
                state.deletion = None;
                card.superseded = false;
            }
            _ => deletion.outcome = Outcome::Unknown,
        }
        self.save(&card, Some(version)).await?;
        Ok(())
    }
}
