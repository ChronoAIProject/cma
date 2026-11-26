//! Type-guard every partial BSON expression; Mongo $and does not short circuit.
use super::*;

const STRINGS: &[(&str, &str)] = &[
    ("owner", "$record.owner"),
    ("sandbox", "$record.sandbox_id"),
    ("workspace", "$record.workspace_id"),
    ("worktree", "$record.worktree_id"),
    ("conversation", "$record.conversation.conversation_id"),
    ("ready_at", "$record.work.ready_at"),
    (
        "runtime_id",
        "$_ownership_retry_parent.record.runtime.runtime_id",
    ),
    (
        "runtime_version",
        "$_ownership_retry_parent.record.runtime.runtime_version",
    ),
    (
        "bound_at",
        "$_ownership_retry_parent.record.runtime.bound_at",
    ),
];
const NUMBERS: &[(&str, &str, i64, i64)] = &[
    (
        "generation",
        "$record.conversation.runtime_generation",
        1,
        i64::MAX,
    ),
    ("attempts", "$record.work.attempts", 0, u32::MAX as i64),
    (
        "command_generation",
        "$agent_command_admission_fence",
        0,
        i64::MAX,
    ),
    (
        "high_water",
        "$_ownership_retry_parent.record.lifecycle.runtime_generation_high_water",
        0,
        i64::MAX,
    ),
];

fn is_type(path: &str, types: &[&str]) -> Document {
    doc! {"$in": [{"$type": path}, types]}
}
fn string(path: &str) -> Document {
    doc! {"$cond": [is_type(path, &["string"]), path, ""]}
}
fn bounded_string(path: &str) -> Document {
    doc! {"$and": [is_type(path, &["string"]),
    {"$gt": [{"$strLenBytes": string(path)}, 0]},
    {"$lte": [{"$strLenBytes": string(path)}, 512]}]}
}
fn number(path: &str, min: i64, max: i64) -> Document {
    doc! {"$and": [is_type(path, &["int", "long"]), {"$gte": [path, min]}, {"$lte": [path, max]}]}
}
fn object(path: &str) -> Document {
    doc! {"$cond": [is_type(path, &["object"]), path, {}]}
}
fn normalized(path: &str) -> Document {
    doc! {"$ifNull": [path, Bson::Null]}
}

fn parent_schema() -> Document {
    use crate::store::work_schema::{
        MANAGED_MACHINE_ORIGIN_VERSION, PREPARED_VERSION, RENEWED_VERSION,
        RUNTIME_IMAGE_ORIGIN_VERSION,
    };
    let schema = "$_ownership_retry_parent.document_schema_version";
    let work = "$_ownership_retry_parent.record.work";
    let protocol = format!("{work}.authority_protocol");
    let proof = format!("{work}.prepared_installation_digest");
    let absent_proof = is_type(&proof, &["missing"]);
    let prepared_proof = doc! {"$or": [is_type(&proof, &["null"]), {"$regexMatch": {
    "input": string(&proof), "regex": "^[0-9a-f]{64}$"}}]};
    let lifecycle = "$_ownership_retry_parent.record.lifecycle";
    let origin = format!("{lifecycle}.runtime_image_origin");
    // Match the retained work reader's version/protocol relation without
    // pulling the complete parent aggregate into every discovery lookup.
    doc! {"$and": [
        is_type(schema, &["int", "long"]), is_type(work, &["object"]),
        {"$or": [is_type(&protocol, &["missing"]), {"$eq": [&protocol, "renewed_v1"]}]},
        {"$or": [
            {"$and": [
                {"$or": [
                    {"$and": [{"$eq": [schema, RUNTIME_IMAGE_ORIGIN_VERSION]},
                        {"$in": [{"$ifNull": [&origin, Bson::Null]}, ["managed_desktop", "user_selected"]]}]},
                    {"$and": [{"$eq": [schema, MANAGED_MACHINE_ORIGIN_VERSION]},
                        {"$eq": [&origin, "managed_machine"]}]},
                ]},
                {"$or": [absent_proof.clone(), prepared_proof.clone()]},
                is_type(&format!("{lifecycle}.custom_runtime"), &["missing"]),
                is_type(&format!("{lifecycle}.runtime_migration_admission"), &["missing"])]},
            {"$and": [{"$eq": [schema, 1]}, is_type(&protocol, &["missing"]), absent_proof.clone()]},
            {"$and": [{"$eq": [schema, RENEWED_VERSION]}, {"$eq": [&protocol, "renewed_v1"]}, absent_proof.clone()]},
            {"$and": [{"$eq": [schema, PREPARED_VERSION]}, {"$not": [absent_proof]}, prepared_proof]},
        ]},
    ]}
}

pub(super) fn active(store: &MongoSandboxStore) -> Document {
    let h = format!("${FIELD}");
    let mut valid = vec![
        doc! {"$eq": [{"$size": {"$objectToArray": object(&h)}}, 11]},
        doc! {"$lte": [{"$bsonSize": object(&h)}, MAX_BYTES as i64]},
        doc! {"$eq": [{"$size": {"$objectToArray": object(&format!("{h}.key"))}}, 15]},
        doc! {"$eq": [format!("{h}.version"), 1]},
        doc! {"$eq": [format!("{h}.retry_classifier_version"), 1]},
        doc! {"$eq": [format!("{h}.owner_protocol"), i64::from(OWNER_PROTOCOL_VERSION)]},
        doc! {"$eq": [format!("{h}.receiver_lease_ms"), RECEIVER_LEASE_MS as i64]},
        doc! {"$in": [format!("{h}.reason"), ["capability_missing", "challenge_protocol_mismatch", "challenge_lifetime_mismatch"]]},
        doc! {"$regexMatch": {"input": string(&format!("{h}.observation")),
        "regex": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"}},
        number(&format!("{h}.owner_epoch"), 1, i64::MAX),
        number(&format!("{h}.claimed_revision"), 1, i64::MAX),
    ];
    for field in [
        "version",
        "retry_classifier_version",
        "owner_protocol",
        "receiver_lease_ms",
    ] {
        valid.push(is_type(&format!("{h}.{field}"), &["int", "long"]));
    }
    for (field, _) in STRINGS {
        valid.push(bounded_string(&format!("{h}.key.{field}")));
    }
    for (field, _, min, max) in NUMBERS {
        valid.push(number(&format!("{h}.key.{field}"), *min, *max));
    }
    let boot = format!("{h}.key.boot_id");
    let provision = format!("{h}.key.provision_generation");
    valid.push(doc! {"$or": [is_type(&boot, &["null"]), bounded_string(&boot)]});
    valid.push(doc! {"$or": [is_type(&provision, &["null"]), number(&provision, 1, i64::MAX)]});
    let observed = format!("{h}.observed_at");
    let until = format!("{h}.until");
    // Both operands are dates before subtraction is evaluated, even for scalar
    // or future-version hints supplied by an older or malformed writer.
    valid.push(doc! {"$cond": [
        {"$and": [is_type(&observed, &["date"]), is_type(&until, &["date"])]},
        {"$and": [{"$eq": [{"$subtract": [&until, &observed]}, HOLD_MS]},
            {"$lte": [&observed, policy_now(store)]}, {"$gt": [&until, policy_now(store)]}]}, false,
    ]});
    doc! {"$and": valid}
}

pub(in crate::store) fn filter(store: &MongoSandboxStore) -> Vec<Document> {
    if !store.ownership_retry_hints_active() {
        return vec![];
    }
    let h = format!("${FIELD}.key");
    let p = "$_ownership_retry_parent.record";
    let mut equal = vec![
        active(store),
        doc! {"$in": ["$record.status", ["ready", "working", "waiting_approval"]]},
        doc! {"$eq": ["$record.work.step", "publish_connection"]},
        doc! {"$eq": [format!("{p}.status"), "active"]},
        doc! {"$eq": [format!("{p}.id"), "$record.sandbox_id"]},
        doc! {"$eq": [format!("{p}.owner"), "$record.owner"]},
        doc! {"$eq": ["$_ownership_retry_parent._id", "$record.sandbox_id"]},
        doc! {"$eq": ["$_ownership_retry_parent.owner", "$record.owner"]},
        doc! {"$eq": ["$_ownership_retry_parent.revision", format!("{p}.revision")]},
        is_type("$_ownership_retry_parent.revision", &["long"]),
        number("$_ownership_retry_parent.revision", 1, i64::MAX),
        number(&format!("{p}.revision"), 1, i64::MAX),
        parent_schema(),
    ];
    for field in ["requested_stop_reason", "deleted_at"] {
        equal.push(doc! {"$eq": [normalized(&format!("$record.{field}")), Bson::Null]});
    }
    for field in [
        "operation",
        "checkpoint_capture",
        "runtime_update",
        "runtime_provision",
        "suspended_runtime",
        "home_recovery",
        "unpublished_recovery",
    ] {
        equal.push(doc! {"$eq": [normalized(&format!("{p}.lifecycle.{field}")), Bson::Null]});
    }
    equal.push(doc! {"$or": [
        {"$eq": [normalized(&format!("{p}.lifecycle.retirement_archive")), Bson::Null]},
        {"$ne": [normalized(&format!("{p}.lifecycle.retirement_archive.verified")), Bson::Null]},
    ]});
    for (field, path) in STRINGS {
        equal.push(doc! {"$eq": [format!("{h}.{field}"), *path]});
    }
    for (field, path, min, max) in NUMBERS {
        let default_zero = matches!(*field, "command_generation" | "high_water");
        equal.push(if default_zero {
            let present = if *field == "command_generation" {
                doc! {"$and": [is_type(path, &["long"]), number(path, *min, *max)]}
            } else {
                number(path, *min, *max)
            };
            doc! {"$or": [is_type(path, &["missing"]), present]}
        } else {
            number(path, *min, *max)
        });
        let current = if default_zero {
            Bson::Document(doc! {"$cond": [is_type(path, &["missing"]), 0_i64, *path]})
        } else {
            Bson::String((*path).into())
        };
        equal.push(doc! {"$eq": [format!("{h}.{field}"), current]});
    }
    for field in ["boot_id", "provision_generation"] {
        equal.push(
            doc! {"$eq": [format!("{h}.{field}"), normalized(&format!("{p}.runtime.{field}"))]},
        );
    }
    let provision = format!("{p}.runtime.provision_generation");
    equal.push(
        doc! {"$or": [is_type(&provision, &["null", "missing"]), number(&provision, 1, i64::MAX)]},
    );
    vec![
        doc! {"$lookup": {"from": store.sandboxes.name(),
        "let": {"id": "$record.sandbox_id", "active": active(store)},
        "pipeline": [{"$match": {"$expr": {"$and": ["$$active", {"$eq": ["$_id", "$$id"]}]}}},
            {"$project": {"_id": 1, "owner": 1, "revision": 1, "document_schema_version": 1,
                "record.id": 1, "record.owner": 1, "record.revision": 1, "record.status": 1,
                "record.work.authority_protocol": 1, "record.work.prepared_installation_digest": 1,
                "record.runtime": 1, "record.lifecycle.operation": 1, "record.lifecycle.checkpoint_capture": 1,
                    "record.lifecycle.runtime_update": 1, "record.lifecycle.runtime_provision": 1,
                    "record.lifecycle.suspended_runtime": 1, "record.lifecycle.home_recovery": 1,
                    "record.lifecycle.unpublished_recovery": 1, "record.lifecycle.retirement_archive": 1,
                    "record.lifecycle.runtime_generation_high_water": 1,
                    "record.lifecycle.runtime_image_origin": 1, "record.lifecycle.custom_runtime": 1,
                    "record.lifecycle.runtime_migration_admission": 1}}],
        "as": "_ownership_retry_parents"}},
        doc! {"$set": {"_ownership_retry_parent": {"$arrayElemAt": ["$_ownership_retry_parents", 0]}}},
        doc! {"$match": {"$expr": {"$not": [{"$and": equal}]}}},
        doc! {"$unset": ["_ownership_retry_parents", "_ownership_retry_parent"]},
    ]
}
