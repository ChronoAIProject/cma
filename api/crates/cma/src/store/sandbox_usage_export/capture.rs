use super::super::sandbox_events::TransactionEventError;
use super::{
    records::{encode, hash, number, Captured},
    *,
};
use layer2_core::sandbox_usage::{SandboxUsage, UsageEvent};
use mongodb::ClientSession;

impl CaptureStore {
    pub(in crate::store) async fn record(
        &self,
        session: &mut ClientSession,
        state: &SandboxUsage,
        event: &UsageEvent,
        sandbox: &layer2_core::sandbox::Sandbox,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        layer2_core::sandbox_usage_evidence::project(event, event.started_at)
            .map_err(|_| Domain(corrupt()))?;
        let identity =
            doc! {"namespace":&self.config.namespace,"owner_issuer":&self.config.owner_issuer};
        let parent = self
            .collection("sandboxes")
            .update_one(
                doc! {
                    "_id":event.sandbox_id.as_str(), "owner":event.owner.as_str(),
                    "$or":[{"usage_capture_identity_v1":{"$exists":false}},
                        {"usage_capture_identity_v1":&identity}],
                },
                doc! {"$inc":{"usage_capture_fence_v1":1_i64},
                "$set":{"usage_capture_identity_v1":identity}},
            )
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        if parent.matched_count != 1 {
            return Err(Domain(corrupt()));
        }
        let (mut stream, fresh) = self.stream(session, state, event).await?;
        let capture = Captured {
            stream_id: stream.id.clone(),
            source: event.clone(),
            previous_revision: (!fresh).then_some(stream.last_revision),
            source_gap: !fresh
                && (stream.last_revision.checked_add(1) != Some(event.revision)
                    || stream.last_ended_at != event.started_at
                    || matches!(
                        event.kind,
                        layer2_core::sandbox_usage::UsageEventKind::Baseline
                    )),
            tracking_started_at: state.tracking_started_at,
            complete_lifecycle: state.complete_lifecycle,
            resource: stream.resource.clone(),
        };
        let bytes = serde_json::to_vec(&capture).map_err(|_| Domain(corrupt()))?;
        self.collection(CAPTURES)
            .insert_one(doc! {
                "_id":event.transaction_id(), "document_schema_version":1_i64,
                "stream_id":&stream.id, "sandbox_id":event.sandbox_id.as_str(),
                "owner":event.owner.as_str(), "revision":number(event.revision).map_err(Domain)?,
                "source_digest":hash(&bytes), "record":encode(&capture).map_err(Domain)?,
                "next_second":number(event.started_at).map_err(Domain)?, "complete":false,
            })
            .session(&mut *session)
            .await
            .map_err(Driver)?;
        stream.last_revision = event.revision;
        stream.last_ended_at = event.ended_at;
        stream.resource = records::ResourceSnapshot::from_sandbox(sandbox);
        let document = stream.document().map_err(Domain)?;
        if fresh {
            self.collection(STREAMS)
                .insert_one(document)
                .session(&mut *session)
                .await
                .map_err(Driver)?;
        } else {
            let updated = self
                .collection(STREAMS)
                .replace_one(doc! {"_id":&stream.id}, document)
                .session(&mut *session)
                .await
                .map_err(Driver)?;
            if updated.matched_count != 1 {
                return Err(Domain(corrupt()));
            }
        }
        Ok(())
    }
}
