//! A consistent, read-only view of the retained release pair.
use mongodb::options::ReadConcern;
use serde::Serialize;

use super::*;
use crate::store::{native_warm_pools, transaction_terminal::GuardedTerminal as _};
use layer1_public::rest::admin::warm_pools::WarmPoolError;

pub(crate) struct TransitionDestination<'a> {
    pub lane: ReleasedLane,
    pub expected_revision: u64,
    pub target_id: &'a str,
}

#[derive(Serialize)]
pub(crate) struct RetainedDestination {
    lane: ReleasedLane,
    lane_revision: u64,
    active_image: String,
    destination_image: String,
    target_id: String,
    target_revision: i64,
    target_replicas: u32,
}

impl MongoReleasedRuntimes {
    pub(crate) async fn inspect_transition_pair(
        &self,
        agent: TransitionDestination<'_>,
        machine: TransitionDestination<'_>,
    ) -> Result<[RetainedDestination; 2], SandboxStoreError> {
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(unavailable)?;
        // The ordinary write-transaction helper can acquire a durable work
        // fence. This command only reads one snapshot and then aborts it.
        session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .await
            .map_err(unavailable)?;
        let result = async {
            let agent = self.inspect_destination(agent, &mut session).await?;
            let machine = self.inspect_destination(machine, &mut session).await?;
            if agent.active_image == agent.destination_image
                && machine.active_image == machine.destination_image
            {
                return Err(SandboxStoreError::Conflict);
            }
            Ok([agent, machine])
        }
        .await;
        let abort = session.abort_guarded().await.map_err(unavailable);
        let destinations = result?;
        abort?;
        Ok(destinations)
    }

    async fn inspect_destination(
        &self,
        proposed: TransitionDestination<'_>,
        session: &mut ClientSession,
    ) -> Result<RetainedDestination, SandboxStoreError> {
        let lane = proposed.lane;
        let document = self
            .lanes
            .find_one(doc! {"_id":self.id(lane)})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::Conflict)?;
        let current = self.verify(lane, document)?.record;
        if current.revision != proposed.expected_revision
            || !current.frozen
            || current.candidate.is_some()
        {
            return Err(SandboxStoreError::Conflict);
        }

        let target = native_warm_pools::transition_target(
            &self.database,
            &self.deployment,
            lane == ReleasedLane::Machine,
            proposed.target_id,
            &mut *session,
        )
        .await
        .map_err(pool_error)?;
        let destination_image = &target.plan.spec.image.uri;
        let release_revision = if destination_image == &current.active.image {
            current.active.revision
        } else {
            let history = self
                .transitions
                .find_one(doc! {
                    "deployment":&self.deployment,
                    "document_schema_version":SCHEMA_VERSION,
                    "transition.lane":lane_key(lane),
                    "transition.from_image":destination_image,
                    "transition.to_revision":{"$lte":revision(current.active.revision)?},
                })
                .session(&mut *session)
                .await
                .map_err(unavailable)?
                .ok_or(SandboxStoreError::Conflict)?;
            self.validate_transition(lane, &history)?;
            history.transition.from_revision
        };
        if release_revision > 1 {
            let promotion = self
                .transitions
                .find_one(doc! {"_id":self.transition_id(lane, release_revision - 1)})
                .session(&mut *session)
                .await
                .map_err(unavailable)?
                .ok_or(SandboxStoreError::Conflict)?;
            self.validate_transition(lane, &promotion)?;
            if promotion.transition.to_image != *destination_image
                || promotion.transition.to_revision != release_revision
            {
                return Err(SandboxStoreError::Conflict);
            }
        }
        native_warm_pools::confirm_transition_target_identity(
            &self.database,
            &target,
            release_revision == 1,
            session,
        )
        .await
        .map_err(pool_error)?;
        Ok(RetainedDestination {
            lane,
            lane_revision: current.revision,
            active_image: current.active.image,
            destination_image: target.plan.spec.image.uri,
            target_id: target.id,
            target_revision: target.revision,
            target_replicas: target.target_replicas,
        })
    }

    fn validate_transition(
        &self,
        lane: ReleasedLane,
        history: &TransitionDocument,
    ) -> Result<(), SandboxStoreError> {
        if history.id != self.transition_id(lane, history.transition.from_revision)
            || history.deployment != self.deployment
            || history.document_schema_version != SCHEMA_VERSION
            || history.transition.lane != lane
            || history.transition.from_revision == 0
            || history.transition.from_revision.checked_add(1)
                != Some(history.transition.to_revision)
        {
            return Err(SandboxStoreError::Corrupt(
                "retained runtime history".into(),
            ));
        }
        Ok(())
    }
}

fn pool_error(error: WarmPoolError) -> SandboxStoreError {
    match error {
        WarmPoolError::Conflict => SandboxStoreError::Conflict,
        _ => SandboxStoreError::Unavailable("retained runtime pool read".into()),
    }
}
