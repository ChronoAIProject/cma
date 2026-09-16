use super::*;

impl MongoChannelControlExecutionStore {
    pub(super) async fn fence_refusal(
        &self,
        snapshot: &ChannelControlRefusalSnapshot,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let invokes = &self.invokes;
        let source = &snapshot.source;
        super::super::super::channel_source::fence(
            invokes,
            &snapshot.source_command,
            &source.sidecar,
            &source.inbox,
            session,
        )
        .await?;
        let database = invokes
            .bridges
            .client
            .database(&invokes.bridges.definitions.namespace().db);
        let intent = &snapshot.admitted.sidecar.intent;
        crate::store::MongoCallbackInbox::fence_accepted_control(&database, intent, session)
            .await?;
        invokes.fence_bridge(&snapshot.bridge, session).await?;
        for (collection, query, fence) in [
            (
                database.collection::<Document>("bridge_channel_control_intents"),
                doc! {"_id":&intent.id,"intent_sha256":intent.fingerprint()?,"record":record_bson(intent).map_err(|_| Error::Corrupt)?},
                "control_refusal_fence",
            ),
            (
                database.collection::<Document>("bridge_channel_control_references"),
                doc! {"_id":&intent.reference.reference,"lookup_sha256":&intent.reference.lookup_sha256,"record":record_bson(&intent.reference).map_err(|_| Error::Corrupt)?},
                // Reference readers require the canonical outer schema and
                // already validate this shared serialization counter.
                "control_admission_fence",
            ),
            (
                database.collection::<Document>("bridge_allocations"),
                doc! {"_id":&source.allocation.id,"revision":source.allocation.revision as i64},
                "control_refusal_fence",
            ),
            (
                database.collection::<Document>("bridge_source_control_policies"),
                doc! {"_id":snapshot.grant.id.as_str(),"record":record_bson(&snapshot.consent).map_err(|_| Error::Corrupt)?},
                "control_refusal_fence",
            ),
            (
                invokes.grants.clone(),
                doc! {"_id":snapshot.grant.id.as_str(),"record":record_bson(&snapshot.grant).map_err(|_| Error::Corrupt)?},
                "control_refusal_fence",
            ),
            (
                invokes.resources.agents.clone(),
                doc! {"_id":snapshot.agent.id.as_str(),"revision":snapshot.agent.revision as i64,
                "creation_fingerprint":&snapshot.agent.creation_fingerprint,"record.created_at":&snapshot.agent.created_at},
                "control_refusal_fence",
            ),
        ] {
            let mut increment = Document::new();
            increment.insert(fence, 1_i64);
            let changed = collection
                .update_one(query, doc! {"$inc":increment})
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(Error::Conflict.into());
            }
        }
        Ok(())
    }
}
