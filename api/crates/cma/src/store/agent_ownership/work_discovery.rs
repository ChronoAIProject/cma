//! Scan past peers' work without starving local work or old claimed steps.
use super::super::sandbox_mongo::MongoSandboxStore;
use super::*;
use layer2_core::agent_ownership::candidates;
use layer2_core::sandbox::store::{AgentWorkDiscovery, SandboxStoreError};
use std::collections::BTreeMap;

impl MongoSandboxStore {
    pub(in crate::store) async fn due_owned_agent_work(
        &self,
        now: &str,
        limit: u16,
        filter: AgentWorkDiscovery<'_>,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        let ownership = self
            .agent_ownership
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let instance = self
            .agent_manager_instance
            .as_deref()
            .ok_or(SandboxStoreError::Conflict)?;
        let membership = ownership.membership().await.map_err(failure)?;
        let mut last = None;
        let mut result = Vec::new();
        loop {
            let mut pipeline =
                super::super::agent_failure_pacing::discovery::filtered_owned_pipeline(
                    self,
                    now,
                    100,
                    last.as_deref(),
                    filter,
                );
            // Fetch claim metadata only for the bounded, paced candidate page.
            // Claims still revalidate the complete current row and exact epoch.
            pipeline.extend([
                doc! {"$lookup": {"from": self.agents.name(), "localField": "_id", "foreignField": "_id",
                    "pipeline": [{"$project": {"_id": 1, "work_lease_expires_at": 1, effects::WORK_EPOCH: 1}}],
                    "as": "owned_candidate"}},
                doc! {"$unwind": "$owned_candidate"},
                doc! {"$replaceWith": "$owned_candidate"},
            ]);
            let rows: Vec<Document> = self
                .agents
                .aggregate(pipeline)
                .await
                .map_err(failure)?
                .try_collect()
                .await
                .map_err(failure)?;
            if rows.is_empty() {
                return Ok(result);
            }
            let ids = rows
                .iter()
                .map(|row| row.get_str("_id").map(str::to_owned))
                .collect::<Result<Vec<_>, _>>()
                .map_err(failure)?;
            last = ids.last().cloned();
            let owners: Vec<Document> = ownership
                .owners
                .find(doc! {"_id": {"$in": &ids}, "instance_id": {"$type": "string"}})
                .await
                .map_err(failure)?
                .try_collect()
                .await
                .map_err(failure)?;
            let owners: BTreeMap<_, _> = owners
                .iter()
                .map(|row| owner_from(row).map(|owner| (owner.token.agent_id.clone(), owner)))
                .collect::<Result<_, _>>()
                .map_err(failure)?;
            for (id, row) in ids.into_iter().zip(rows) {
                let id = AgentId(id);
                let owner = owners.get(&id);
                if let Some(owner) = owner.filter(|owner| owner.expires_at_ms > membership.now_ms) {
                    if owner.token.instance_id != instance {
                        continue;
                    }
                    let same_claim = row.get_i64(effects::WORK_EPOCH).ok()
                        == i64::try_from(owner.token.epoch).ok();
                    if same_claim
                        && row
                            .get_str("work_lease_expires_at")
                            .is_ok_and(|expiry| expiry > now)
                    {
                        continue;
                    }
                } else if candidates(
                    &id,
                    &membership,
                    owner.map(|owner| owner.token.instance_id.as_str()),
                )
                .map_err(failure)?
                .first()
                .is_none_or(|member| member.instance_id != instance)
                {
                    continue;
                }
                result.push(id);
                if result.len() >= usize::from(limit) {
                    return Ok(result);
                }
            }
        }
    }
}

fn failure(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
