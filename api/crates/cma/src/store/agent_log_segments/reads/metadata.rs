use super::*;
use crate::store::agent_log_segments::capture::preview_budget::PreviewBudget;

pub(in crate::store) async fn read_head(
    store: &MongoSandboxStore,
    aggregate: &Document,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Head> {
    read_head_from(&database(store), aggregate, session, snapshot, None, None).await
}

pub(in crate::store::agent_log_segments) async fn read_export_head(
    store: &MongoSandboxStore,
    aggregate: &Document,
    session: &mut ClientSession,
    max_entries: i64,
    max_bytes: i64,
) -> Result<Head> {
    read_head_from(
        &database(store),
        aggregate,
        session,
        true,
        Some((max_entries, max_bytes)),
        None,
    )
    .await
}

pub(in crate::store::agent_log_segments) async fn read_head_from(
    db: &Database,
    aggregate: &Document,
    session: &mut ClientSession,
    snapshot: bool,
    tail_budget: Option<(i64, i64)>,
    mut budget: Option<&mut PreviewBudget>,
) -> Result<Head> {
    let agent = crate::store::agents::read_agent(aggregate)?;
    if aggregate.contains_key(QUARANTINE_FIELD) {
        return Err(corrupt().into());
    }
    let (incarnation, revision) = read_format(aggregate)?.ok_or_else(corrupt)?;
    let audits = majority_collection::<Document>(db, AUDITS);
    if one(
        &audits,
        doc! {"agent_id":agent.id.as_str(),"incarnation":incarnation,"status":"corrupt"},
        Some(doc! {"_id":1}),
        None,
        session,
        snapshot,
        budget.as_deref_mut(),
    )
    .await?
    .is_some()
    {
        return Err(SandboxStoreError::Corrupt(
            "Agent log is quarantined by an integrity audit".into(),
        )
        .into());
    }

    let heads = majority_collection(db, HEADS);
    let head = Head::read(
        one(
            &heads,
            doc! {"_id":agent.id.as_str()},
            None,
            None,
            session,
            snapshot,
            budget.as_deref_mut(),
        )
        .await?
        .ok_or_else(corrupt)?,
    )?;
    if head.agent_id != agent.id.as_str()
        || head.incarnation != incarnation
        || head.revision != revision
        || u64::try_from(head.last_seq).ok() != Some(agent.conversation.last_seq)
    {
        return Err(corrupt().into());
    }
    if tail_budget
        .is_some_and(|(entries, bytes)| head.active_count > entries || head.active_bytes > bytes)
    {
        return Err(SandboxStoreError::AgentLogCapacityExceeded.into());
    }
    let seals = majority_collection::<Document>(db, SEGMENTS);
    if one(&seals, doc! {"agent_id":&head.agent_id,"incarnation":&head.incarnation,"ordinal":{"$gte":head.active_ordinal}}, Some(doc! {"_id":1}), None, session, snapshot, budget.as_deref_mut()).await?.is_some()
    {
        return Err(corrupt().into());
    }
    if head.active_ordinal == 0 && head.last_seq > 0 {
        validate_unsealed_creation(db, &agent, session, snapshot, budget.as_deref_mut()).await?;
    }
    super::super::source_keys::prove_budgeted(db, &head, session, snapshot, budget).await?;
    Ok(head)
}

/// Until creation belongs to an immutable seal, even a page that does not
/// deliver that row must bind its lineage to the captured Agent.
pub(super) async fn validate_unsealed_creation(
    db: &Database,
    agent: &layer2_core::sandbox::Agent,
    session: &mut ClientSession,
    snapshot: bool,
    budget: Option<&mut PreviewBudget>,
) -> Result<()> {
    let source = majority_collection::<Document>(db, AGENT_LOG);
    let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
    let row = if let Some(budget) = budget {
        budget
            .document(
                &source,
                doc! {"agent_id":agent.id.as_str(),"seq":1_i64},
                None,
                session,
            )
            .await?
            .ok_or_else(corrupt)?
    } else {
        crate::store::stale_turns::find_with_proof_budget(
            &source,
            doc! {"agent_id":agent.id.as_str(),"seq":1_i64},
            session,
            snapshot,
        )
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(corrupt)?
    };
    read.document(
        mongodb::bson::serialize_to_vec(&row)
            .map_err(|_| corrupt())?
            .len(),
    );
    let entry = read_entry(&row)?;
    if entry.agent_id != agent.id
        || entry.seq != 1
        || !matches!(&entry.kind,
        AgentLogKind::Created { workspace_id, worktree_id }
        if workspace_id == &agent.workspace_id && worktree_id == agent.creation_worktree_id())
    {
        return Err(corrupt().into());
    }
    read.complete();
    Ok(())
}

pub(in crate::store::agent_log_segments) async fn seal_at(
    store: &MongoSandboxStore,
    head: &Head,
    ordinal: i64,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Seal> {
    seal_at_from(&database(store), head, ordinal, session, snapshot, None).await
}

pub(in crate::store::agent_log_segments) async fn seal_at_from(
    db: &Database,
    head: &Head,
    ordinal: i64,
    session: &mut ClientSession,
    snapshot: bool,
    budget: Option<&mut PreviewBudget>,
) -> Result<Seal> {
    let seals = majority_collection(db, SEGMENTS);
    let seal = Seal::read(
        one(
            &seals,
            doc! {"agent_id":&head.agent_id,"incarnation":&head.incarnation,"ordinal":ordinal},
            None,
            None,
            session,
            snapshot,
            budget,
        )
        .await?
        .ok_or_else(corrupt)?,
    )?;
    if seal.agent_id != head.agent_id
        || seal.incarnation != head.incarnation
        || seal.ordinal != ordinal
    {
        return Err(corrupt().into());
    }
    Ok(seal)
}

pub(in crate::store::agent_log_segments) async fn containing(
    store: &MongoSandboxStore,
    head: &Head,
    seq: i64,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Seal> {
    containing_from(&database(store), head, seq, session, snapshot, None).await
}

pub(super) async fn containing_from(
    db: &Database,
    head: &Head,
    seq: i64,
    session: &mut ClientSession,
    snapshot: bool,
    mut budget: Option<&mut PreviewBudget>,
) -> Result<Seal> {
    let seals = majority_collection(db, SEGMENTS);
    let seal = Seal::read(one(&seals, doc! {"agent_id":&head.agent_id,"incarnation":&head.incarnation,"start_seq":{"$lte":seq}}, None, Some(doc! {"start_seq":-1}), session, snapshot, budget.as_deref_mut()).await?.ok_or_else(corrupt)?)?;
    if seal.agent_id != head.agent_id
        || seal.incarnation != head.incarnation
        || seal.start_seq > seq
        || seal.end_seq < seq
        || seal.ordinal >= head.active_ordinal
    {
        return Err(corrupt().into());
    }
    if seal.ordinal > 0 {
        let previous = seal_at_from(
            db,
            head,
            seal.ordinal - 1,
            session,
            snapshot,
            budget.as_deref_mut(),
        )
        .await?;
        seal.follows(&previous)?;
    }
    if seal.ordinal + 1 == head.active_ordinal {
        seal.precedes(head)?;
    } else {
        let next = seal_at_from(db, head, seal.ordinal + 1, session, snapshot, budget).await?;
        next.follows(&seal)?;
    }
    Ok(seal)
}

async fn one(
    source: &mongodb::Collection<Document>,
    filter: Document,
    projection: Option<Document>,
    sort: Option<Document>,
    session: &mut ClientSession,
    snapshot: bool,
    budget: Option<&mut PreviewBudget>,
) -> Result<Option<Document>> {
    if let Some(budget) = budget {
        return budget.document(source, filter, sort, session).await;
    }
    if projection.is_none() && sort.is_none() {
        return crate::store::stale_turns::find_with_proof_budget(
            source, filter, session, snapshot,
        )
        .await
        .map_err(TransactionLogError::Driver);
    }
    let mut find = source.find_one(filter);
    if let Some(projection) = projection {
        find = find.projection(projection);
    }
    if let Some(sort) = sort {
        find = find.sort(sort);
    }
    if snapshot {
        find = find.read_concern(ReadConcern::snapshot());
    }
    find.session(session)
        .await
        .map_err(TransactionLogError::Driver)
}

pub(in crate::store::agent_log_segments) async fn preview_head(
    store: &MongoSandboxStore,
    row: &Document,
    session: &mut ClientSession,
    budget: &mut PreviewBudget,
) -> Result<Head> {
    read_head_from(&database(store), row, session, true, None, Some(budget)).await
}
pub(in crate::store::agent_log_segments) async fn preview_seal_at(
    store: &MongoSandboxStore,
    head: &Head,
    ordinal: i64,
    session: &mut ClientSession,
    budget: &mut PreviewBudget,
) -> Result<Seal> {
    seal_at_from(&database(store), head, ordinal, session, true, Some(budget)).await
}
pub(in crate::store::agent_log_segments) async fn preview_containing(
    store: &MongoSandboxStore,
    head: &Head,
    seq: i64,
    session: &mut ClientSession,
    budget: &mut PreviewBudget,
) -> Result<Seal> {
    containing_from(&database(store), head, seq, session, true, Some(budget)).await
}
