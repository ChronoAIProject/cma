use super::{records::*, *};
use crate::store::sandbox_maintenance::evidence::{read_queued_control, sessions};

pub(super) struct Captured {
    pub vector: RuntimeActivityVector,
    pub desktop: RuntimeActivityDesktop,
    pub storage_incarnation: String,
    pub started: i64,
    pub completed: i64,
}

pub(super) async fn snapshot(
    store: &MongoSandboxStore,
    id: &SandboxId,
    context: &RuntimeActivitySourceContext,
    session: &mut ClientSession,
    snapshot_read: bool,
) -> Result<RuntimeActivitySnapshot, ApplyError> {
    let current = current(store, id, context, session, snapshot_read).await?;
    let watermark = session.snapshot_time().ok_or_else(invalid)?;
    let snapshot = RuntimeActivitySnapshot {
        source: context.clone(),
        capture_started_at_ms: current.started,
        capture_completed_at_ms: current.completed,
        watermark: RuntimeActivityWatermark {
            seconds: watermark.time,
            increment: watermark.increment,
        },
        storage_incarnation: current.storage_incarnation,
        desktop: current.desktop,
        vector: current.vector,
    };
    snapshot.validate().map_err(ApplyError::Domain)?;
    Ok(snapshot)
}

pub(super) async fn current(
    store: &MongoSandboxStore,
    id: &SandboxId,
    context: &RuntimeActivitySourceContext,
    session: &mut ClientSession,
    snapshot_read: bool,
) -> Result<Captured, ApplyError> {
    let started = now_ms()?;
    let storage_incarnation = storage::incarnation(store).await?;
    let before = desktop(id)?;
    if before.process_incarnation != context.process_incarnation {
        return Err(invalid());
    }
    let mut roots = bounded_find(
        &store.sandboxes,
        doc! {"_id":id.as_str()},
        projection::sandbox(),
        session,
        snapshot_read,
    )
    .await?;
    let root = roots
        .pop()
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let mut sandbox = aggregate(&root, id, false)?;
    sandbox.active |= read_queued_control(store, id, session, snapshot_read).await?;
    if sandbox.last_seq.is_none() {
        return Err(invalid());
    }
    let workspace_rows = bounded_find(
        &store.workspaces,
        doc! {"$or":[{"sandbox_id":id.as_str()},{"record.sandbox_id":id.as_str()}]},
        projection::workspace(),
        session,
        snapshot_read,
    )
    .await?;
    let agent_rows = bounded_find(
        &store.agents,
        doc! {"$or":[{"sandbox_id":id.as_str()},{"record.sandbox_id":id.as_str()}]},
        projection::agent(),
        session,
        snapshot_read,
    )
    .await?;
    let mut vector = RuntimeActivityVector {
        sandbox_id: id.clone(),
        sandbox,
        workspaces: workspace_rows
            .iter()
            .map(|row| aggregate(row, id, false))
            .collect::<Result<_, _>>()?,
        agents: agent_rows
            .iter()
            .map(|row| aggregate(row, id, true))
            .collect::<Result<_, _>>()?,
        command_orders: vec![],
        commands: vec![],
        log_heads: vec![],
        log_tails: vec![],
        desktop_leases: vec![],
    };
    for agent in &agent_rows {
        capture_agent_sources(store, agent, &mut vector, session, snapshot_read).await?;
    }
    let leases = bounded_find(
        &store.sandbox_sessions,
        doc! {"sandbox_id":id.as_str()},
        sessions::projection(),
        session,
        snapshot_read,
    )
    .await?;
    for mut lease in leases {
        let expired = sessions::expired(&lease, id, started).ok_or_else(invalid)?;
        let key =
            layer2_core::profiles::content_digest(&text(&lease, "_id")?).map_err(|_| invalid())?;
        lease.insert("_id", key);
        lease.remove("database_now");
        vector.desktop_leases.push(record(&lease, !expired)?);
    }
    for rows in [
        &mut vector.command_orders,
        &mut vector.commands,
        &mut vector.log_heads,
        &mut vector.log_tails,
        &mut vector.desktop_leases,
    ] {
        rows.sort_by(|a, b| a.id.cmp(&b.id));
    }
    let after = desktop(id)?;
    if before != after {
        return Err(invalid());
    }
    if storage_incarnation != storage::incarnation(store).await? {
        return Err(invalid());
    }
    vector.validate().map_err(ApplyError::Domain)?;
    Ok(Captured {
        vector,
        desktop: after,
        storage_incarnation,
        started,
        completed: now_ms()?,
    })
}

async fn capture_agent_sources(
    store: &MongoSandboxStore,
    agent: &Document,
    vector: &mut RuntimeActivityVector,
    session: &mut ClientSession,
    snapshot_read: bool,
) -> Result<(), ApplyError> {
    let id = text(agent, "_id")?;
    let orders = bounded_find(
        &store.agent_command_orders,
        doc! {"_id":id},
        projection::order(),
        session,
        snapshot_read,
    )
    .await?;
    let mut last_order = 0_i64;
    if let Some(order) = orders.first() {
        let lane =
            crate::store::agent_commands::read_command_lane(order, id).map_err(|_| invalid())?;
        last_order = i64::try_from(lane.last_order).map_err(|_| invalid())?;
        vector
            .command_orders
            .push(record(order, lane.lease.is_some())?);
    }
    // Stable last-order/next-fence plus qualified writer policy covers transient
    // command admission/execution. Retain all current work and the final order;
    // never scan prompt bodies or an unbounded lifetime ledger.
    let scope = doc! {"$or":[{"agent_id":id},{"record.receipt.envelope.agent_id":id}]};
    let filter = if last_order == 0 {
        scope
    } else {
        // Select malformed/unknown states as well as pending work. Looking only
        // at a positive pending-state allowlist could hide an inconsistent row.
        doc! {"$and":[scope, {"$or":[
            {"status":{"$nin":["completed","failed","cancelled"]}},
            {"record.status":{"$nin":["completed","failed","cancelled"]}},
            {"agent_order":{"$gte":last_order}}
        ]}]}
    };
    let commands = bounded_find(
        &store.agent_commands,
        filter,
        projection::command(),
        session,
        snapshot_read,
    )
    .await?;
    if last_order > 0
        && !commands
            .iter()
            .any(|row| number(row, "agent_order").ok() == Some(last_order as u64))
    {
        return Err(invalid());
    }
    for command_row in commands {
        if vector.commands.len() == RUNTIME_ACTIVITY_MAX_ROWS
            || text(&command_row, "agent_id")? != id
            || last_order == 0
            || number(&command_row, "agent_order")? > last_order as u64
        {
            return Err(invalid());
        }
        vector.commands.push(command(&command_row)?);
    }
    let seq = number(agent, "last_seq")?;
    if let Some((incarnation, revision)) =
        crate::store::agent_log_segments::read_format(agent).map_err(ApplyError::Domain)?
    {
        let heads = bounded_find(
            &store.segment_logs.heads,
            doc! {"_id":id},
            projection::head(),
            session,
            snapshot_read,
        )
        .await?;
        let head_row = heads.first().ok_or_else(invalid)?;
        let head = crate::store::agent_log_segments::Head::read(head_row.clone())
            .map_err(ApplyError::Domain)?;
        let expected = crate::store::agent_log_segments::incarnation_fields(
            id,
            text(agent, "creation_key")?,
            text(agent, "record.created_at")?,
        );
        if head.incarnation != incarnation
            || head.incarnation != expected
            || head.revision != revision
            || head.last_seq != i64::try_from(seq).map_err(|_| invalid())?
        {
            return Err(invalid());
        }
        vector.log_heads.push(record(head_row, false)?);
    } else {
        let logs = bounded_find(&store.agent_log,
            doc! {"$or":[{"agent_id":id,"seq":{"$gte":i64::try_from(seq).map_err(|_| invalid())?}},
                {"record.agent_id":id,"record.seq":{"$gte":i64::try_from(seq).map_err(|_| invalid())?}}]},
            projection::log_tail(), session, snapshot_read).await?;
        if (seq == 0 && !logs.is_empty()) || (seq != 0 && logs.len() != 1) {
            return Err(invalid());
        }
        if let Some(tail) = logs.first() {
            crate::store::sandbox_mongo::require_version(tail).map_err(|_| invalid())?;
            same(tail, "agent_id", "record.agent_id")?;
            same(tail, "seq", "record.seq")?;
            if text(tail, "agent_id")? != id || number(tail, "seq")? != seq {
                return Err(invalid());
            }
            let mut tail_record = record(tail, false)?;
            tail_record.id = id.into();
            tail_record.last_seq = Some(seq);
            vector.log_tails.push(tail_record);
        }
    }
    Ok(())
}
