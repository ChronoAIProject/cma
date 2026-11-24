use super::{Census, Member, Request, Result, Row, INVALID, MAX_SOURCE};
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{
    agents::read_agent, sandboxes::read_sandbox, workspaces::read_workspace,
    worktrees::read_worktree,
};
use mongodb::{
    bson::{doc, Document},
    options::Hint,
    Client, ClientSession, Database,
};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub(in crate::store) async fn observe(
    client: &Client,
    database: &Database,
    request: &Request,
    request_sha256: String,
) -> Result<Census> {
    request.validate()?;
    let mut session = client.start_session().await.map_err(|_| INVALID)?;
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .map_err(|_| INVALID)?;
    let result = read(database, request, request_sha256, &mut session).await;
    // Success never commits. A failed/unknown read never publishes a partial census.
    session.abort_guarded().await.map_err(|_| INVALID)?;
    result
}

async fn read(
    database: &Database,
    request: &Request,
    request_sha256: String,
    session: &mut ClientSession,
) -> Result<Census> {
    let mut rows = Vec::with_capacity(request.agents.len());
    let mut used = 0;
    let mut source = Sha256::new();
    for member in &request.agents {
        let mut hash = Sha256::new();
        let mut reader = Reader {
            database,
            session,
            used: &mut used,
            hash: &mut hash,
        };
        let (outcome, assessment) = classify(&mut reader, member).await?;
        let row_hash = format!("{:x}", hash.finalize());
        source.update(row_hash.as_bytes());
        rows.push(Row {
            member: member.clone(),
            outcome,
            assessment,
            source_sha256: row_hash,
        });
    }
    Ok(Census {
        format: "runtime-ledger-census-v1",
        observed_at: layer2_core::now_rfc3339(),
        request_sha256,
        source_sha256: format!("{:x}", source.finalize()),
        rows,
        writes: false,
        recovery_authorized: false,
    })
}

type Classification = (
    &'static str,
    Option<layer2_core::sandbox::ledger_census::LedgerCensusAssessment>,
);

async fn classify(reader: &mut Reader<'_>, member: &Member) -> Result<Classification> {
    let Some(document) = reader.get("agents", &member.agent_id).await? else {
        return Ok(("agent_missing_preserve", None));
    };
    if document.contains_key(crate::store::agent_log_integrity::QUARANTINE_FIELD) {
        return Ok(("agent_quarantined_preserve", None));
    }
    let Ok(agent) = read_agent(&document) else {
        return Ok(("agent_unreadable_preserve", None));
    };
    if agent.id.as_str() != member.agent_id || agent.sandbox_id.as_str() != member.sandbox_id {
        return Ok(("cohort_binding_changed_preserve", None));
    }
    let Some(document) = reader.get("sandboxes", &member.sandbox_id).await? else {
        return Ok(("parent_missing_preserve", None));
    };
    let Ok(parent) = read_sandbox(&document) else {
        return Ok(("parent_unreadable_preserve", None));
    };
    if agent.owner != parent.owner {
        return Ok(("lineage_changed_preserve", None));
    }
    let Some(document) = reader
        .get("workspaces", agent.workspace_id.as_str())
        .await?
    else {
        return Ok(("workspace_missing_preserve", None));
    };
    let Ok(workspace) = read_workspace(&document) else {
        return Ok(("workspace_unreadable_preserve", None));
    };
    let Some(document) = reader.get("worktrees", agent.worktree_id.as_str()).await? else {
        return Ok(("worktree_missing_preserve", None));
    };
    let Ok(worktree) = read_worktree(&document) else {
        return Ok(("worktree_unreadable_preserve", None));
    };
    match agent.ledger_census(&parent, &workspace, &worktree) {
        Ok(assessment) => Ok(("observed_preserve", Some(assessment))),
        Err(_) => Ok(("lineage_or_state_invalid_preserve", None)),
    }
}

struct Reader<'a> {
    database: &'a Database,
    session: &'a mut ClientSession,
    used: &'a mut usize,
    hash: &'a mut Sha256,
}

impl Reader<'_> {
    async fn get(&mut self, collection: &str, id: &str) -> Result<Option<Document>> {
        let document = self
            .database
            .collection::<Document>(collection)
            .find_one(doc! {"_id":id})
            .hint(Hint::Name("_id_".into()))
            .max_time(Duration::from_secs(5))
            .session(&mut *self.session)
            .await
            .map_err(|_| INVALID)?;
        let bytes = match &document {
            Some(value) => mongodb::bson::serialize_to_vec(value).map_err(|_| INVALID)?,
            None => Vec::new(),
        };
        *self.used = self.used.checked_add(bytes.len()).ok_or(INVALID)?;
        if *self.used > MAX_SOURCE {
            return Err(INVALID);
        }
        for value in [collection.as_bytes(), id.as_bytes(), &bytes] {
            self.hash.update((value.len() as u64).to_be_bytes());
            self.hash.update(value);
        }
        Ok(document)
    }
}
