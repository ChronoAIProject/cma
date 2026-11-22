use super::super::sandbox_events::TransactionEventError;
use super::{records::Stream, *};
use layer2_core::sandbox_usage::{SandboxUsage, UsageEvent};
use mongodb::ClientSession;

impl CaptureStore {
    pub(super) async fn stream(
        &self,
        session: &mut ClientSession,
        state: &SandboxUsage,
        event: &UsageEvent,
    ) -> Result<(Stream, bool), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let existing = self
            .collection(STREAMS)
            .find_one(doc! {
                "namespace":&self.config.namespace, "sandbox_id":event.sandbox_id.as_str(),
                "created_at":records::number(state.created_at).map_err(Domain)?,
            })
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        if let Some(existing) = existing {
            let stream = Stream::read(&existing).map_err(Domain)?;
            if stream.owner != event.owner.as_str()
                || stream.owner_issuer != self.config.owner_issuer
                || stream.tracking_started_at != state.tracking_started_at
                || event.revision <= stream.last_revision
                || event.started_at < stream.last_ended_at
            {
                return Err(Domain(corrupt()));
            }
            return Ok((stream, false));
        }
        Ok((
            Stream {
                id: uuid::Uuid::new_v4().to_string(),
                source_epoch: uuid::Uuid::new_v4().to_string(),
                namespace: self.config.namespace.clone(),
                owner: event.owner.as_str().into(),
                owner_issuer: self.config.owner_issuer.clone(),
                sandbox_id: event.sandbox_id.as_str().into(),
                created_at: state.created_at,
                tracking_started_at: state.tracking_started_at,
                capture_start: event.started_at,
                first_revision: event.revision,
                last_revision: event.revision,
                last_ended_at: event.ended_at,
                complete_lifecycle: state.complete_lifecycle && event.revision == 1,
                sequence: 0,
                previous_digest: None,
                projected_revision: None,
                resource: Default::default(),
            },
            true,
        ))
    }
}
