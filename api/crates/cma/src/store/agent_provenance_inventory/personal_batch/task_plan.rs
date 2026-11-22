//! Emit complete task evidence individually, including oversized originals.
use super::*;
use layer2_core::sandbox::{Agent, AgentStatus};

pub(super) fn append(store: &MongoSandboxStore, agents: &[Agent], pipeline: &mut Vec<Document>) {
    let ready = agents
        .iter()
        .filter(|agent| agent.status == AgentStatus::Ready)
        .map(|agent| agent.id.as_str())
        .collect::<Vec<_>>();
    if ready.is_empty() {
        return;
    }
    let start = vec![
        doc! {"$match":{"_id":{"$in":ready}}},
        doc! {"$project":{"_id":1,"last_seq":1}},
    ];
    let latest = vec![
        doc! {"$match":{"record.kind.type":{"$in":["created","turn_started","turn_completed","turn_failed","turn_interrupted","conversation_gap"]}}},
        doc! {"$sort":{"seq":-1}},
        doc! {"$limit":1},
    ];
    let mut branch = start.clone();
    branch.push(lookup(store.agent_log.name(), "latest", latest.clone()));
    finish(store, branch, "latest", "task_latest", pipeline);

    let mut branch = start.clone();
    let mut latest_hint = latest;
    latest_hint
        .push(doc! {"$project":{"seq":1,"record.kind.type":1,"record.kind.turn_sequence":1}});
    branch.push(lookup(store.agent_log.name(), "latest", latest_hint));
    branch.push(doc! {"$set":{"latest":{"$arrayElemAt":["$latest",0]}}});
    branch.push(doc! {"$lookup":{
        "from":store.agent_log.name(),"localField":"_id","foreignField":"agent_id","as":"gap",
        "let":{"latest":"$latest","head":"$last_seq"},
        "pipeline":[
            {"$match":{"record.kind.type":"conversation_gap","$expr":{"$and":[
                {"$eq":["$$latest.record.kind.type","turn_completed"]},
                {"$lte":["$$latest.seq","$$head"]},
                {"$gt":["$seq","$$latest.record.kind.turn_sequence"]},
                {"$lt":["$seq","$$latest.seq"]}
            ]}}}, {"$sort":{"seq":-1}}, {"$limit":1}
        ]
    }});
    finish(store, branch, "gap", "task_gap", pipeline);

    let mut branch = start;
    branch.push(lookup(
        store.agent_commands.name(),
        "pending",
        vec![
            doc! {"$match":{"status":{"$in":["queued","in_progress","waiting_action"]}}},
            doc! {"$limit":1},
        ],
    ));
    finish(store, branch, "pending", "task_pending", pipeline);
}

fn lookup(collection: &str, field: &str, pipeline: Vec<Document>) -> Document {
    doc! {"$lookup":{"from":collection,"localField":"_id","foreignField":"agent_id","as":field,"pipeline":pipeline}}
}

fn finish(
    store: &MongoSandboxStore,
    mut branch: Vec<Document>,
    field: &str,
    kind: &str,
    pipeline: &mut Vec<Document>,
) {
    let field = format!("${field}");
    branch.push(doc! {"$unwind":&field});
    branch.push(doc! {"$replaceWith":field});
    branch.extend(personal_inventory::query::frame(kind));
    pipeline.push(doc! {"$unionWith":{"coll":store.agents.name(),"pipeline":branch}});
}
