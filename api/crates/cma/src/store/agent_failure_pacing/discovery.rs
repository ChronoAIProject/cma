//! Filter held IDs before the page limit; optional ownership hints join their parent.
use super::*;
use layer2_core::sandbox::store::{AgentWorkDiscovery, StartupWorkFilter};

#[cfg(test)]
pub(in crate::store) fn pipeline(
    store: &MongoSandboxStore,
    now: &str,
    limit: u16,
    holds: &std::collections::BTreeSet<layer2_core::AgentId>,
) -> Vec<Document> {
    filtered_pipeline(
        store,
        now,
        limit,
        AgentWorkDiscovery::new(holds, StartupWorkFilter::All),
    )
}

pub(in crate::store) fn filtered_pipeline(
    store: &MongoSandboxStore,
    now: &str,
    limit: u16,
    filter: AgentWorkDiscovery<'_>,
) -> Vec<Document> {
    build(store, now, limit, None, false, filter)
}

/// Ownership recovery may replace a previous epoch's live worker claim. Keep
/// the same pacing/wakeup predicates and paginate before choosing local owners.
#[cfg(test)]
pub(in crate::store) fn owned_pipeline(
    store: &MongoSandboxStore,
    now: &str,
    limit: u16,
    after: Option<&str>,
    holds: &std::collections::BTreeSet<layer2_core::AgentId>,
) -> Vec<Document> {
    filtered_owned_pipeline(
        store,
        now,
        limit,
        after,
        AgentWorkDiscovery::new(holds, StartupWorkFilter::All),
    )
}

pub(in crate::store) fn filtered_owned_pipeline(
    store: &MongoSandboxStore,
    now: &str,
    limit: u16,
    after: Option<&str>,
    filter: AgentWorkDiscovery<'_>,
) -> Vec<Document> {
    build(store, now, limit, after, true, filter)
}

fn build(
    store: &MongoSandboxStore,
    now: &str,
    limit: u16,
    after: Option<&str>,
    include_claimed: bool,
    filter: AgentWorkDiscovery<'_>,
) -> Vec<Document> {
    let latest = layer2_core::seconds_from_rfc3339(now)
        .map(|seconds| {
            layer2_core::rfc3339_at(
                SystemTime::UNIX_EPOCH + Duration::from_secs(seconds.saturating_add(MAX_DELAY)),
            )
        })
        .unwrap_or_else(|| now.to_owned());
    let hint = format!("${FIELD}");
    let candidate = doc! {"$and": [
        {"$in": [format!("{hint}.class"), [CLASS, PARENT_CLASS]]},
        {"$eq": ["$record.work.step", "publish_connection"]},
        {"$eq": [format!("{hint}.ready_at"), "$record.work.ready_at"]},
        {"$eq": [format!("{hint}.attempts"), "$record.work.attempts"]},
        {"$gte": [format!("{hint}.repeats"), 0]},
        {"$lte": [format!("{hint}.repeats"), 4]},
        {"$gt": [format!("{hint}.until"), now]},
        {"$lte": [format!("{hint}.until"), &latest]},
    ]};
    let mut available = doc! {
        // Discovery supplies candidates only. The claim path validates the
        // exact envelope and current parents before granting a writer lease.
        "document_schema_version": {"$in": [1_i64, 2_i64]}, "work_pending": true,
    };
    let mut excluded = Vec::new();
    if !filter.receiver_holds.is_empty() {
        // Both union arms inherit this predicate before their sort and limit.
        // An explicit control on a held ID remains eligible immediately.
        let ids: Vec<_> = filter.receiver_holds.iter().map(|id| id.as_str()).collect();
        let steps = layer2_core::sandbox::AgentWorkStep::RECEIVER_PACED
            .map(|step| step.operation_name())
            .to_vec();
        excluded.push(doc! {"_id": {"$in": ids}, "record.work.step": {"$in": steps}});
    }
    if let Some(cooldowns) = filter.acquisition_cooldowns {
        // Both arms match the current task before their limits. A replacement
        // task on the same ID, including a new Stop or Delete, is eligible.
        excluded.extend(cooldowns.iter().filter_map(|(id, work)| {
            let mut exact = doc! {
                "_id": id.as_str(),
                "record.status": work.status.name(),
                "record.work.step": work.step.operation_name(),
                "record.work.ready_at": &work.ready_at,
                "record.work.attempts": i64::from(work.attempts),
                "record.work.next_fence": i64::try_from(work.next_fence).ok()?,
            };
            if let Some(revision) = work.control_revision {
                exact.insert("record.revision", i64::try_from(revision).ok()?);
            }
            Some(exact)
        }));
    }
    match filter.startup {
        StartupWorkFilter::All => {}
        StartupWorkFilter::Exclude(sandboxes) => {
            let ids: Vec<_> = sandboxes.iter().map(|id| id.as_str()).collect();
            let mut closed = startup_candidate();
            closed.insert("record.sandbox_id", doc! {"$in": ids});
            excluded.push(closed);
        }
        StartupWorkFilter::Probe(sandbox_id) => {
            available.extend(startup_candidate());
            available.insert("record.sandbox_id", sandbox_id.as_str());
        }
    }
    if !excluded.is_empty() {
        available.insert("$nor", excluded);
    }
    if !include_claimed {
        available.insert(
            "$or",
            vec![
                doc! {"work_lease_expires_at": Bson::Null},
                doc! {"work_lease_expires_at": {"$lte": now}},
            ],
        );
    }
    if let Some(after) = after {
        available.insert("_id", doc! {"$gt": after});
    }
    let mut ordinary = available.clone();
    ordinary.insert("work_ready_at", doc! {"$lte": now});
    ordinary.insert("$expr", doc! {"$not": [candidate.clone()]});
    let mut pipeline = vec![doc! {"$match": ordinary}];
    pipeline.extend(crate::store::agent_inspection_park::filter(store, false));
    pipeline.extend(super::ownership::discovery::filter(store));
    pipeline.extend(page(limit));

    // Only future paced work needs the three exact parent-index probes. Both
    // arms have their own LIMIT before their bounded union is sorted again.
    let mut paced = available;
    paced.insert(format!("{FIELD}.until"), doc! {"$gt": now, "$lte": latest});
    paced.insert("$expr", candidate);
    let projection = if store.ownership_retry_hints_active() {
        doc! {"_id": 1, "revision": 1, FIELD: 1, "agent_command_admission_fence": 1,
        super::ownership::FIELD: 1, "record.owner": 1, "record.sandbox_id": 1,
        "record.workspace_id": 1, "record.worktree_id": 1, "record.status": 1,
        "record.conversation.conversation_id": 1, "record.conversation.runtime_generation": 1,
        "record.work.step": 1, "record.work.ready_at": 1, "record.work.attempts": 1,
        "record.requested_stop_reason": 1, "record.deleted_at": 1}
    } else {
        doc! {"_id": 1, "revision": 1, FIELD: 1, "agent_command_admission_fence": 1,
        "record.sandbox_id": 1, "record.workspace_id": 1, "record.worktree_id": 1}
    };
    let mut wake = vec![doc! {"$match": paced}];
    wake.extend(crate::store::agent_inspection_park::filter(store, false));
    wake.push(doc! {"$project": projection});
    let mut holds = vec![
        Bson::Document(doc! {
            "$eq": [format!("{hint}.agent_revision"), "$revision"]
        }),
        Bson::Document(doc! {
            "$eq": [
                {"$ifNull": [format!("{hint}.command_admission_generation"), 0_i64]},
                {"$ifNull": ["$agent_command_admission_fence", 0_i64]},
            ]
        }),
    ];
    for (name, collection) in [
        ("sandbox", &store.sandboxes),
        ("workspace", &store.workspaces),
        ("worktree", &store.worktrees),
    ] {
        let output = format!("pacing_{name}");
        wake.push(doc! {"$lookup": {
            "from": collection.name(), "localField": format!("record.{name}_id"), "foreignField": "_id",
            "pipeline": [{"$match": crate::store::work_schema::for_collection(collection,
                doc! {"document_schema_version": DOCUMENT_SCHEMA_VERSION})},
                {"$project": {"revision": 1, "record.revision": 1}}], "as": &output,
        }});
        for path in ["revision", "record.revision"] {
            holds.push(Bson::Document(doc! {"$eq": [
                {"$ifNull": [{"$arrayElemAt": [format!("${output}.{path}"), 0]}, -1_i64]},
                format!("{hint}.{name}_revision"),
            ]}));
        }
    }
    wake.push(doc! {"$match": {"$expr": {"$not": [{"$and": holds}]}}});
    wake.extend(super::ownership::discovery::filter(store));
    wake.extend(page(limit));
    pipeline.push(doc! {"$unionWith": {"coll": store.agents.name(), "pipeline": wake}});
    // An advisory read race between arms may yield one ID twice; claims remain
    // authoritative, but discovery still promises unique, sorted IDs.
    pipeline.push(doc! {"$group": {"_id": "$_id"}});
    pipeline.extend(page(limit));
    pipeline
}

fn startup_candidate() -> Document {
    let steps = StartupWorkFilter::STEPS.map(|step| step.operation_name());
    // An Agent with uncertain runtime work, or an explicit control step, must
    // remain visible even when its siblings are waiting for the same Sandbox.
    doc! {
        "record.work.step": {"$in": steps},
        "record.work.lease": Bson::Null,
        "record.work.cleanup_debt.0": {"$exists": false},
    }
}

fn page(limit: u16) -> [Document; 3] {
    [
        doc! {"$sort": {"_id": 1}},
        doc! {"$limit": i64::from(limit)},
        doc! {"$project": {"_id": 1}},
    ]
}
