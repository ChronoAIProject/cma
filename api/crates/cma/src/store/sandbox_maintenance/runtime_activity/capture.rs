use super::{records::*, *};

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
    let storage_incarnation = storage_incarnation(store).await?;
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
    let sandbox = aggregate(&root, id, false)?;
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
        projection::session(),
        session,
        snapshot_read,
    )
    .await?;
    for mut lease in leases {
        if text(&lease, "sandbox_id")? != id.as_str()
            || !(crate::store::sandbox_mongo::require_version(&lease).is_ok()
                || crate::store::sandbox_sessions::is_legacy_v1(&lease))
        {
            return Err(invalid());
        }
        let expiry = layer2_core::seconds_from_rfc3339(text(&lease, "lease_expires_at")?)
            .ok_or_else(invalid)?;
        let key =
            layer2_core::profiles::content_digest(&text(&lease, "_id")?).map_err(|_| invalid())?;
        lease.insert("_id", key);
        let active = i64::try_from(expiry)
            .ok()
            .and_then(|at| at.checked_mul(1000))
            .ok_or_else(invalid)?
            > started;
        vector.desktop_leases.push(record(&lease, active)?);
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
    if storage_incarnation != self::storage_incarnation(store).await? {
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
        let lane = command_lane(order, id)?;
        last_order = i64::try_from(lane.last_order).map_err(|_| invalid())?;
        vector
            .command_orders
            .push(record(order, lane.lease.is_some())?);
    }
    // Stable last-order/next-fence plus qualified writer policy covers transient
    // command admission/execution. Retain all current work and the final order;
    // never scan prompt bodies or an unbounded lifetime ledger.
    let commands = bounded_find(&store.agent_commands,
        doc! {"$and":[{"$or":[{"agent_id":id},{"record.receipt.envelope.agent_id":id}]},
            {"$or":[{"status":{"$in":["queued","in_progress","waiting_action"]}},{"agent_order":last_order}]}]},
        projection::command(), session, snapshot_read).await?;
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

async fn storage_incarnation(store: &MongoSandboxStore) -> Result<String, ApplyError> {
    let hello = store
        .client
        .database("admin")
        .run_command(doc! {"hello":1})
        .await
        .map_err(ApplyError::Driver)?;
    let process = get(&hello, "topologyVersion.processId")
        .and_then(mongodb::bson::Bson::as_object_id)
        .ok_or_else(invalid)?;
    layer2_core::profiles::content_digest(&(
        text(&hello, "setName")?,
        process.to_hex(),
        store.sandboxes.namespace().db,
    ))
    .map_err(|_| invalid())
}

fn command_lane(
    row: &Document,
    id: &str,
) -> Result<layer2_core::sandbox::agent_io::CommandLane, ApplyError> {
    use layer2_core::sandbox::agent_io::CommandLane;
    crate::store::sandbox_mongo::require_version(row).map_err(|_| invalid())?;
    let last_order = number(row, "last_order")?;
    let next_fence = if row.contains_key("next_fence") {
        number(row, "next_fence")?
    } else {
        1
    };
    let lease: Option<layer2_core::sandbox::agent_io::CommandLease> = if present(row, "lease") {
        Some(
            mongodb::bson::deserialize_from_bson(row.get("lease").ok_or_else(invalid)?.clone())
                .map_err(|_| invalid())?,
        )
    } else {
        None
    };
    if text(row, "_id")? != id
        || last_order == 0
        || next_fence == 0
        || lease.as_ref().is_some_and(|lease| {
            lease.claim.agent_id.as_str() != id
                || lease.claim.fence == 0
                || lease.claim.fence >= next_fence
                || layer2_core::seconds_from_rfc3339(&lease.expires_at).is_none()
        })
    {
        return Err(invalid());
    }
    Ok(CommandLane {
        last_order,
        next_fence,
        lease,
    })
}
