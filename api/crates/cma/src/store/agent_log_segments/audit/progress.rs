use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::options::ReadConcern;

pub(super) async fn current(
    database: &Database,
    agent_id: &str,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<(Agent, Head), SandboxStoreError> {
    let agents = collection(database, "agents");
    let query = agents.find_one(doc! {"_id":agent_id});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let row = query
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    let agent = crate::store::agents::read_agent(&row)?;
    let (incarnation, revision) = super::super::read_format(&row)?.ok_or_else(corrupt)?;
    let heads = collection(database, HEADS);
    let query = heads.find_one(doc! {"_id":agent_id});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let head = Head::read(
        query
            .session(session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?,
    )?;
    if head.agent_id != agent_id
        || head.incarnation != incarnation
        || head.revision != revision
        || head.last_seq as u64 != agent.conversation.last_seq
    {
        return Err(corrupt());
    }
    Ok((agent, head))
}

pub(super) async fn claim(
    client: &Client,
    database: &Database,
    agent_id: &str,
    audit_id: &str,
    cancel: bool,
) -> Result<Progress, SandboxStoreError> {
    if agent_id.is_empty() || agent_id.len() > 512 || audit_id.is_empty() || audit_id.len() > 128 {
        return Err(SandboxStoreError::InvalidPage);
    }
    let mut session = client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = claim_transaction(database, agent_id, audit_id, cancel, &mut session).await;
    match result {
        Ok(progress) => {
            commit(&mut session).await?;
            Ok(progress)
        }
        Err(error) => {
            let _ = session.abort_guarded().await;
            Err(error)
        }
    }
}
async fn claim_transaction(
    database: &Database,
    agent_id: &str,
    audit_id: &str,
    cancel: bool,
    session: &mut ClientSession,
) -> Result<Progress, SandboxStoreError> {
    let key = id(agent_id, audit_id);
    let audits = collection(database, AUDITS);
    let old = audits
        .find_one(doc! {"_id":&key})
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut progress = if let Some(row) = old.as_ref() {
        read(row.clone(), agent_id, audit_id)?
    } else {
        let (agent, head) = current(database, agent_id, session, false).await?;
        Progress {
            id: key.clone(),
            format_version: 2,
            agent_id: agent_id.into(),
            incarnation: head.incarnation.clone(),
            audit_id: audit_id.into(),
            holder: String::new(),
            expires_at_ms: 0,
            fence: 0,
            step: 0,
            status: "running".into(),
            through_seq: head.last_seq,
            head_revision: head.revision,
            result_generation: format!("audit-result:{}", uuid::Uuid::new_v4().simple()),
            captured_head: head,
            captured_agent: agent,
            completed: Checkpoint {
                next_ordinal: 0,
                checked_entries: 0,
                checked_bytes: 0,
                previous_seal: None,
                semantic: Default::default(),
            },
            stage: None,
        }
    };
    if progress.status != "running" {
        return Ok(progress);
    }
    let at = now();
    if !cancel && progress.expires_at_ms > at {
        return Err(SandboxStoreError::Conflict);
    }
    let expected_fence = progress.fence;
    let expected_step = progress.step;
    progress.fence = progress.fence.checked_add(1).ok_or_else(corrupt)?;
    progress.holder = uuid::Uuid::new_v4().to_string();
    progress.expires_at_ms = at.checked_add(LEASE_MS).ok_or_else(corrupt)?;
    // A partially collected unit is never promoted. Keep the completed prefix
    // and abandon only that unit's private generation after a crash/reclaim.
    if matches!(progress.stage, Some(Stage::Collecting { .. })) {
        progress.stage = None;
    }
    if cancel {
        progress.status = "cancelled".into();
        progress.expires_at_ms = 0;
        progress.stage = None;
    }
    let row = progress.document()?;
    if old.is_some() {
        let mut filter =
            doc! {"_id":&key,"fence":expected_fence,"step":expected_step,"status":"running"};
        if !cancel {
            filter.insert("expires_at_ms", doc! {"$lte":at});
        }
        if audits
            .replace_one(filter, row)
            .session(session)
            .await
            .map_err(unavailable)?
            .matched_count
            != 1
        {
            return Err(SandboxStoreError::Conflict);
        }
    } else {
        audits
            .insert_one(row)
            .session(session)
            .await
            .map_err(unavailable)?;
    }
    Ok(progress)
}

pub(super) fn read(row: Document, agent: &str, audit: &str) -> Result<Progress, SandboxStoreError> {
    if bson::serialize_to_vec(&row).map_err(|_| corrupt())?.len() > MAX_PROGRESS_BYTES {
        return Err(corrupt());
    }
    for field in [
        "format_version",
        "expires_at_ms",
        "fence",
        "step",
        "through_seq",
        "head_revision",
    ] {
        row.get_i64(field).map_err(|_| corrupt())?;
    }
    checkpoint_types(row.get_document("completed").map_err(|_| corrupt())?)?;
    Head::read(
        row.get_document("captured_head")
            .map_err(|_| corrupt())?
            .clone(),
    )?;
    if let Ok(stage) = row.get_document("stage") {
        if stage
            .get_str("phase")
            .is_ok_and(|phase| phase == "promoting")
        {
            checkpoint_types(stage.get_document("completed").map_err(|_| corrupt())?)?;
        }
    }
    let progress: Progress = bson::deserialize_from_document(row).map_err(|_| corrupt())?;
    progress.captured_head.validate()?;
    let checkpoint = &progress.completed;
    if progress.format_version != 2
        || progress.id != id(agent, audit)
        || progress.agent_id != agent
        || progress.audit_id != audit
        || progress.incarnation != digest::incarnation(&progress.captured_agent)
        || progress.captured_agent.id.as_str() != agent
        || progress.captured_head.agent_id != agent
        || progress.captured_head.incarnation != progress.incarnation
        || progress.through_seq != progress.captured_head.last_seq
        || progress.head_revision != progress.captured_head.revision
        || progress.captured_agent.conversation.last_seq != progress.through_seq as u64
        || progress.fence < 0
        || progress.step < 0
        || progress.result_generation.is_empty()
        || progress.result_generation.len() > 160
        || progress.result_generation == progress.captured_head.evidence_generation
        || checkpoint.next_ordinal < 0
        || checkpoint.next_ordinal > progress.captured_head.active_ordinal + 1
        || checkpoint.checked_entries < 0
        || checkpoint.checked_entries > progress.through_seq
        || checkpoint.checked_bytes < 0
        || checkpoint.checked_bytes > progress.captured_head.lifetime_bytes
        || checkpoint.semantic.through_seq != checkpoint.checked_entries as u64
        || !matches!(
            progress.status.as_str(),
            "running" | "complete" | "installed" | "cancelled" | "corrupt"
        )
        || (progress.status != "running" && progress.stage.is_some())
    {
        return Err(corrupt());
    }
    if let Some(stage) = &progress.stage {
        if stage.generation().is_empty()
            || stage.generation().len() > 160
            || stage.generation() == progress.result_generation
            || stage.generation() == progress.captured_head.evidence_generation
        {
            return Err(corrupt());
        }
        if let Stage::Promoting {
            completed,
            tail,
            cursor,
            ..
        } = stage
        {
            if completed.next_ordinal != checkpoint.next_ordinal + 1
                || completed.checked_entries < checkpoint.checked_entries
                || completed.checked_entries > progress.through_seq
                || completed.checked_entries - checkpoint.checked_entries > MAX_ENTRIES
                || completed.checked_bytes < checkpoint.checked_bytes
                || completed.checked_bytes > progress.captured_head.lifetime_bytes
                || completed.checked_bytes - checkpoint.checked_bytes > MAX_BYTES
                || completed.semantic.through_seq != completed.checked_entries as u64
                || *tail != (completed.next_ordinal == progress.captured_head.active_ordinal + 1)
                || cursor
                    .as_ref()
                    .is_some_and(|s| s.is_empty() || s.len() > 2048)
            {
                return Err(corrupt());
            }
        }
    }
    Ok(progress)
}

fn checkpoint_types(row: &Document) -> Result<(), SandboxStoreError> {
    for field in ["next_ordinal", "checked_entries", "checked_bytes"] {
        row.get_i64(field).map_err(|_| corrupt())?;
    }
    Ok(())
}
