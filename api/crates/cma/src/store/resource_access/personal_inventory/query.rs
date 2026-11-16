//! Deduplicate identifiers before fetching complete parent records.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::store) fn plan(store: &MongoSandboxStore, agents: &[Agent]) -> Vec<Document> {
    if agents.is_empty() {
        return vec![doc! {"$match":{"_id":{"$in":[]}}}];
    }
    let limit = agents.len();
    let filter = ids(agents.iter().map(|agent| agent.id.as_str()));
    let mut pipeline = selection(filter.clone(), limit);
    pipeline.extend(frame("agent"));
    // These identifiers are candidate hints only. Complete current records are
    // fetched on this snapshot; every Agent must still equal its selected row,
    // and every parent relationship is revalidated before anything is released.
    for (kind, filter, collection) in [
        (
            "worktree",
            ids(agents.iter().map(|a| a.worktree_id.as_str())),
            &store.worktrees,
        ),
        (
            "workspace",
            ids(agents.iter().map(|a| a.workspace_id.as_str())),
            &store.workspaces,
        ),
        (
            "sandbox",
            ids(agents.iter().map(|a| a.sandbox_id.as_str())),
            &store.sandboxes,
        ),
        (
            "policy",
            ids(agents.iter().map(|a| a.sandbox_id.as_str())),
            &store.resource_policies,
        ),
    ] {
        let mut branch = selection(filter, limit);
        branch.extend(frame(kind));
        pipeline.push(doc! {"$unionWith":{"coll":collection.name(),"pipeline":branch}});
    }
    let mut parents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for agent in agents {
        parents
            .entry(agent.sandbox_id.as_str())
            .or_default()
            .push(agent.id.as_str());
    }
    // Query equality deliberately includes array-valued projections. They must
    // reach the complete decoder and fail just as on the singular read path.
    let clauses = parents
        .into_iter()
        .map(|(sandbox, ids)| {
            doc! {
                "sandbox_id":sandbox, "$or":[
                    {"selector_kind":"sandbox","selector_id":sandbox},
                    {"selector_kind":"agent","selector_id":{"$in":ids}}
                ]
            }
        })
        .collect::<Vec<_>>();
    let mut shares = selection(
        doc! {"revoked":false,"$or":clauses},
        limit * (MAX_ACTIVE_RESOURCE_SHARES + 1),
    );
    shares.extend(frame("share"));
    pipeline.push(doc! {"$unionWith":{"coll":store.resource_shares.name(),"pipeline":shares}});
    pipeline
}

fn selection(filter: Document, limit: usize) -> Vec<Document> {
    vec![
        doc! {"$match":filter},
        doc! {"$sort":{"_id":1}},
        doc! {"$limit":limit as i64},
    ]
}

fn ids<'a>(values: impl Iterator<Item = &'a str>) -> Document {
    doc! {"_id":{"$in":values.collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>()}}
}

pub(in crate::store) fn frame(kind: &str) -> [Document; 3] {
    [
        doc! {"$project":{"_id":0,"frames":frames(kind,"$$ROOT")}},
        doc! {"$unwind":"$frames"},
        doc! {"$replaceWith":"$frames"},
    ]
}

fn frames(kind: &str, source: &str) -> Document {
    // Most records use one tagged row. Near Mongo's BSON ceiling, put the tag
    // in a separate row so the original record still fits without truncation.
    doc! {"$cond":[
        {"$gt":[{"$bsonSize":source},16*1024*1024-256]},
        [{"personal_inventory_source":kind},source],
        [{"personal_inventory_source":kind,"value":source}]
    ]}
}
