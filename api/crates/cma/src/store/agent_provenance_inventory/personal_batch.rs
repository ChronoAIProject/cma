//! Personal provenance evidence shares one snapshot; no serial source fallback.
use super::*;
use crate::store::resource_access::personal_inventory;
use layer2_core::sandbox::{channel_usage::AgentChannelUsageState, store::AgentProvenanceSnapshot};
use mongodb::bson::Document;
use std::collections::BTreeMap;

mod task_plan;

pub(super) async fn read(
    store: &MongoSandboxStore,
    subject: &layer2_core::SubjectId,
    query: &AgentListQuery,
    page: &PageRequest,
) -> Result<Vec<AgentChannelUsageSnapshot>, SandboxStoreError> {
    capture(store, subject, query, page)
        .await
        .map_err(|error| match error {
            ReadError::Driver(_) => unavailable(),
            ReadError::Domain(ResourceAccessStoreError::Conflict) => SandboxStoreError::Conflict,
            ReadError::Domain(error) => SandboxStoreError::Corrupt(error.to_string()),
        })
}

#[tracing::instrument(
    level = "debug",
    target = "cma.agent_inventory",
    name = "capture",
    skip_all
)]
async fn capture(
    store: &MongoSandboxStore,
    subject: &layer2_core::SubjectId,
    query: &AgentListQuery,
    page: &PageRequest,
) -> Result<Vec<AgentChannelUsageSnapshot>, ReadError> {
    let mut session = store.client.start_session().snapshot(true).await?;
    let collection = crate::store::snapshot_collection::collection(&store.agents, true);
    let mut cursor = collection
        .aggregate(personal_selection(subject, query, page)?)
        .comment("cma.personal_agent_selection")
        .batch_size(u32::from(page.limit) + 2)
        .max_time(Duration::from_secs(3))
        .session(&mut session)
        .await?;
    let mut telemetry = crate::store::mongo_read_telemetry::Read::find(collection.name());
    let mut agents = Vec::new();
    while let Some(document) = telemetry.next_in_session(&mut cursor, &mut session).await? {
        agents.push(
            tracing::debug_span!(target: "cma.agent_inventory", "record_decode")
                .in_scope(|| read_agent(&document))
                .map_err(|_| ResourceAccessStoreError::Corrupt)?,
        );
    }
    telemetry.complete();
    if agents.is_empty() {
        return Ok(Vec::new());
    }
    let mut evidence = Evidence::default();
    let resources = personal_inventory::read_captured_sources(
        store,
        &agents,
        &mut session,
        pipeline(store, &agents),
        |kind, document| evidence.insert(kind, document, agents.len()),
    )
    .await?;
    if resources.len() != agents.len()
        || resources
            .iter()
            .zip(&agents)
            .any(|(row, agent)| row.lineage.agent.as_ref() != Some(agent))
    {
        return Err(ResourceAccessStoreError::Conflict.into());
    }
    let rows = resources
        .into_iter()
        .map(|resource| evidence.snapshot(resource))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn pipeline(store: &MongoSandboxStore, agents: &[layer2_core::sandbox::Agent]) -> Vec<Document> {
    let ids = agents
        .iter()
        .map(|agent| agent.id.as_str())
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    for (kind, collection, filter) in [
        (
            "creation",
            &store.agent_creation_provenance,
            doc! {"_id":{"$in":&ids}},
        ),
        (
            "first_log",
            &store.agent_log,
            doc! {"agent_id":{"$in":&ids},"seq":1_i64},
        ),
        (
            "usage",
            &store.agent_channel_usage,
            doc! {"_id":{"$in":&ids}},
        ),
    ] {
        let mut branch = vec![
            doc! {"$match":filter},
            doc! {"$limit":agents.len() as i64 + 1},
        ];
        branch.extend(personal_inventory::query::frame(kind));
        result.push(doc! {"$unionWith":{"coll":collection.name(),"pipeline":branch}});
    }
    // The state record alone is never sufficient: choose the actual latest
    // event by the existing (Agent, revision) index, including orphan events.
    // Capture decoded these IDs on the same snapshot used for this lookup.
    let selected = ids
        .into_iter()
        .map(|id| doc! {"_id":id})
        .collect::<Vec<_>>();
    let mut latest = vec![
        doc! {"$documents":{"$literal":selected}},
        doc! {"$lookup":{"from":store.channel_usage_events.name(),"localField":"_id","foreignField":"agent_id","as":"event","pipeline":[{"$sort":{"revision":-1}},{"$limit":1}]}},
        doc! {"$unwind":"$event"},
        doc! {"$replaceWith":"$event"},
    ];
    latest.extend(personal_inventory::query::frame("usage_event"));
    result.push(doc! {"$unionWith":{"pipeline":latest}});
    task_plan::append(store, agents, &mut result);
    result
}

#[derive(Default)]
struct Evidence {
    creation: BTreeMap<String, Document>,
    first: BTreeMap<String, Document>,
    usage: BTreeMap<String, Document>,
    events: BTreeMap<String, Document>,
    latest: BTreeMap<String, Document>,
    gap: BTreeMap<String, Document>,
    pending: BTreeMap<String, Document>,
}
impl Evidence {
    fn insert(
        &mut self,
        kind: &str,
        document: Document,
        limit: usize,
    ) -> Result<(), ResourceAccessStoreError> {
        let (map, key) = match kind {
            "creation" => (&mut self.creation, "_id"),
            "first_log" => (&mut self.first, "agent_id"),
            "usage" => (&mut self.usage, "_id"),
            "usage_event" => (&mut self.events, "agent_id"),
            "task_latest" => (&mut self.latest, "agent_id"),
            "task_gap" => (&mut self.gap, "agent_id"),
            "task_pending" => (&mut self.pending, "agent_id"),
            _ => return Err(ResourceAccessStoreError::Corrupt),
        };
        let id = document
            .get_str(key)
            .map_err(|_| ResourceAccessStoreError::Corrupt)?
            .to_owned();
        if map.insert(id, document).is_some() || map.len() > limit {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(())
    }
    #[tracing::instrument(
        level = "debug",
        target = "cma.agent_inventory",
        name = "evidence_decode",
        skip_all
    )]
    fn snapshot(
        &self,
        resource: layer2_core::resource_access::ResourceAccessSnapshot,
    ) -> Result<AgentChannelUsageSnapshot, ResourceAccessStoreError> {
        let id = resource
            .lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?
            .id
            .clone();
        let corrupt = |_| ResourceAccessStoreError::Corrupt;
        let provenance = AgentProvenanceSnapshot {
            creation: self
                .creation
                .get(id.as_str())
                .map(crate::store::agent_provenance::read)
                .transpose()
                .map_err(corrupt)?,
            first_log: self
                .first
                .get(id.as_str())
                .map(crate::store::agent_log::read_entry)
                .transpose()
                .map_err(corrupt)?,
            resource,
        };
        let stored = self
            .usage
            .get(id.as_str())
            .map(crate::store::agent_channel_usage::read_state)
            .transpose()
            .map_err(corrupt)?;
        let latest = self
            .events
            .get(id.as_str())
            .map(crate::store::agent_channel_usage::read_event)
            .transpose()
            .map_err(corrupt)?;
        let state = match (stored, latest) {
            (Some(stored), Some(latest)) => {
                stored.validate_latest(&latest).map_err(corrupt)?;
                if stored.state.creation.agent_id != id {
                    return Err(ResourceAccessStoreError::Corrupt);
                }
                stored.state
            }
            (None, None) => {
                AgentChannelUsageState::empty(provenance.creation_provenance(&id).map_err(corrupt)?)
            }
            _ => return Err(ResourceAccessStoreError::Corrupt),
        };
        let latest = self
            .latest
            .get(id.as_str())
            .map(crate::store::agent_log::read_entry)
            .transpose()
            .map_err(corrupt)?;
        let gap = self
            .gap
            .get(id.as_str())
            .map(crate::store::agent_log::read_entry)
            .transpose()
            .map_err(corrupt)?;
        let pending = self.pending.get(id.as_str());
        if let Some(document) = pending {
            crate::store::agent_commands::read_command(document)
                .map_err(|_| ResourceAccessStoreError::Corrupt)?;
        }
        let task_status = layer2_core::sandbox::AgentTaskStatus::from_latest(
            provenance
                .resource
                .lineage
                .agent
                .as_ref()
                .ok_or(ResourceAccessStoreError::Corrupt)?,
            latest.as_ref(),
            gap.as_ref(),
            pending.is_some(),
        );
        let snapshot = AgentChannelUsageSnapshot {
            provenance,
            state,
            task_status,
        };
        snapshot.validate(&id).map_err(corrupt)?;
        Ok(snapshot)
    }
}
