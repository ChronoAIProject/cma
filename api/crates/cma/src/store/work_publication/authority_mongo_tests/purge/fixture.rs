use super::*;
use futures_util::TryStreamExt;

pub(super) struct PurgeFixture {
    pub f: Fixture,
    pub claimed: Sandbox,
}

pub(super) type State = Vec<Vec<Document>>;

impl PurgeFixture {
    pub async fn new() -> Self {
        let f = Fixture::new().await;
        for id in ["sbx_purge_contract", "sbx_unrelated"] {
            let mut parent = sandbox(id);
            parent.status = SandboxStatus::Retired;
            parent.credential.delivered = false;
            parent.credential.current = None;
            parent.work = SandboxWorkState::new(SandboxWorkStep::Purge, MINUTE.into());
            parent.retired_at = Some(MINUTE.into());
            parent.purge_after = Some(MINUTE.into());
            f.independent.create_sandbox(parent).await.unwrap();
            let agent = format!("agent-{id}");
            // Legacy v1 projected children remain purgeable without segment markers.
            for (rows, key) in [
                (&f.independent.workspaces, format!("workspace-{id}")),
                (&f.independent.worktrees, format!("worktree-{id}")),
                (&f.independent.agents, agent.clone()),
                (&f.independent.agent_reuse_bindings, format!("reuse-{id}")),
                (&f.independent.checkpoints, format!("checkpoint-{id}")),
                (&f.independent.sandbox_events, format!("event-{id}")),
            ] {
                rows.insert_one(
                    doc! {"_id":&key,"document_schema_version":1_i32,"sandbox_id":id,"name":&key,
                    "workspace_id":format!("workspace-{id}"),"worktree_id":format!("worktree-{id}"),
                    "owner":"synthetic-owner","creation_key":&key},
                )
                .await
                .unwrap();
            }
            for (rows, key) in [
                (&f.independent.agent_log, format!("log-{id}")),
                (&f.independent.suggestions, format!("suggestion-{id}")),
            ] {
                rows.insert_one(doc! {"_id":key,"document_schema_version":1_i32,"agent_id":&agent})
                    .await
                    .unwrap();
            }
        }
        let claimed = f
            .independent
            .claim_sandbox_work(
                &layer2_core::SandboxId("sbx_purge_contract".into()),
                "purge-worker",
                now(),
                work_lease_deadline(),
            )
            .await
            .unwrap()
            .unwrap();
        Self { f, claimed }
    }

    pub async fn state(&self) -> State {
        state(&self.f.independent).await
    }

    pub fn removed(&self, before: &State) -> State {
        let root = self.claimed.id.as_str();
        let agent = format!("agent-{root}");
        before
            .iter()
            .map(|rows| {
                rows.iter()
                    .filter(|row| {
                        row.get_str("_id").ok() != Some(root)
                            && row.get_str("sandbox_id").ok() != Some(root)
                            && row.get_str("agent_id").ok() != Some(agent.as_str())
                    })
                    .cloned()
                    .collect()
            })
            .collect()
    }
}

pub(super) async fn state(store: &MongoSandboxStore) -> State {
    let mut result = Vec::new();
    for rows in [
        &store.sandboxes,
        &store.workspaces,
        &store.worktrees,
        &store.agents,
        &store.checkpoints,
        &store.agent_reuse_bindings,
        &store.sandbox_events,
        &store.agent_log,
        &store.suggestions,
    ] {
        result.push(
            rows.find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    result
}
