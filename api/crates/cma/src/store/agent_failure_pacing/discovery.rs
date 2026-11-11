//! Filter held IDs before the page limit; ordinary work never joins parent rows.
use super::*;

pub(in crate::store) fn pipeline(
    store: &MongoSandboxStore,
    now: &str,
    limit: u16,
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
        {"$eq": [format!("{hint}.class"), CLASS]},
        {"$eq": ["$record.work.step", "publish_connection"]},
        {"$eq": [format!("{hint}.ready_at"), "$record.work.ready_at"]},
        {"$eq": [format!("{hint}.attempts"), "$record.work.attempts"]},
        {"$gte": [format!("{hint}.repeats"), 0]},
        {"$lte": [format!("{hint}.repeats"), 4]},
        {"$gt": [format!("{hint}.until"), now]},
        {"$lte": [format!("{hint}.until"), &latest]},
    ]};
    let available = doc! {
        "document_schema_version": DOCUMENT_SCHEMA_VERSION, "work_pending": true,
        "$or": [{"work_lease_expires_at": Bson::Null}, {"work_lease_expires_at": {"$lte": now}}],
    };
    let mut ordinary = available.clone();
    ordinary.insert("work_ready_at", doc! {"$lte": now});
    ordinary.insert("$expr", doc! {"$not": [candidate.clone()]});
    let mut pipeline = vec![doc! {"$match": ordinary}];
    pipeline.extend(page(limit));

    // Only future paced work needs the three exact parent-index probes. Both
    // arms have their own LIMIT before their bounded union is sorted again.
    let mut paced = available;
    paced.insert(format!("{FIELD}.until"), doc! {"$gt": now, "$lte": latest});
    paced.insert("$expr", candidate);
    let mut wake = vec![
        doc! {"$match": paced},
        doc! {"$project": {"_id": 1, "revision": 1, FIELD: 1, "agent_command_admission_fence": 1,
        "record.sandbox_id": 1, "record.workspace_id": 1, "record.worktree_id": 1}},
    ];
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
            "pipeline": [{"$match": {"document_schema_version": DOCUMENT_SCHEMA_VERSION}},
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
    wake.extend(page(limit));
    pipeline.push(doc! {"$unionWith": {"coll": store.agents.name(), "pipeline": wake}});
    // An advisory read race between arms may yield one ID twice; claims remain
    // authoritative, but discovery still promises unique, sorted IDs.
    pipeline.push(doc! {"$group": {"_id": "$_id"}});
    pipeline.extend(page(limit));
    pipeline
}

fn page(limit: u16) -> [Document; 3] {
    [
        doc! {"$sort": {"_id": 1}},
        doc! {"$limit": i64::from(limit)},
        doc! {"$project": {"_id": 1}},
    ]
}
