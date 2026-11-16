//! Deduplicate identifiers before fetching complete parent records.
use super::*;
use std::collections::BTreeSet;

pub(super) fn plan(store: &MongoSandboxStore, agents: &[Agent]) -> Vec<Document> {
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
    let mut shares = selection(filter, limit);
    shares.push(doc!{"$lookup":{
        "from":store.resource_shares.name(), "localField":"record.sandbox_id", "foreignField":"sandbox_id",
        "let":{"agent":"$_id", "sandbox":"$record.sandbox_id"},
        "pipeline":[{"$match":{"revoked":false,"$or":[
            {"selector_kind":"sandbox","$expr":selector("$$sandbox")},
            {"selector_kind":"agent","$expr":selector("$$agent")}
        ]}},{"$sort":{"_id":1}},{"$limit":MAX_ACTIVE_RESOURCE_SHARES as i64 + 1}],"as":"source"
    }});
    shares.extend([
        doc!{"$unwind":"$source"},
        doc!{"$group":{"_id":"$source._id","source":{"$first":"$source"},"agents":{"$addToSet":"$_id"}}},
        doc!{"$project":{"_id":0,"frames":[
            {"personal_inventory_source":"share_targets"},{"_id":"$_id","agents":"$agents"},
            {"personal_inventory_source":"share"},"$source"
        ]}},
        doc!{"$unwind":"$frames"}, doc!{"$replaceWith":"$frames"},
    ]);
    pipeline.push(doc! {"$unionWith":{"coll":store.agents.name(),"pipeline":shares}});
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

fn frame(kind: &str) -> [Document; 3] {
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

// Match the established query's scalar-or-array equality. A malformed selected
// projection must reach the complete share decoder and fail, never disappear.
fn selector(target: &str) -> Document {
    doc! {"$cond":[{"$isArray":"$selector_id"},{"$in":[target,"$selector_id"]},{"$eq":["$selector_id",target]}]}
}
