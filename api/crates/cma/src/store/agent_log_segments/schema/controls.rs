//! Additive shapes for the writer's exact durable control records.
use mongodb::bson::{doc, Document};

fn object(properties: Document) -> Document {
    let required: Vec<_> = properties.keys().cloned().collect();
    doc! {"bsonType":"object","required":required,
    "additionalProperties":false,"properties":properties}
}

fn long(minimum: i64) -> Document {
    doc! {"bsonType":"long","minimum":minimum}
}

fn string(maximum: i32) -> Document {
    doc! {"bsonType":"string","minLength":1,"maxLength":maximum}
}

fn proof(name: &str) -> Document {
    object(doc! {"name":{"enum":[name]},"revision":long(1)})
}

pub(super) fn gate() -> Document {
    let cohort = doc! {"bsonType":"array","maxItems":64,"uniqueItems":true,
    "items":string(512)};
    let policy = object(doc! {"enabled":{"bsonType":"bool"},
    "agents":cohort.clone(),"owners":cohort,
    "cohort_seed":{"bsonType":"string","maxLength":128},
    "basis_points":{"bsonType":"int","minimum":0,"maximum":10_000}});
    let slot = object(
        doc! {"holder":string(1200),"fingerprint":{"bsonType":"binData"},
        "fence":long(1),"expires_at":{"bsonType":"date"}},
    );
    let shape = object(doc! {
        "_id":{"enum":["agent_log_segments_v1","agent_log_capacity_recovery_v1"]},
        "format_version":{"bsonType":"long","enum":[1_i64]},
        "revision":long(1),"admission_fence":long(0),"policy":policy,
        "slots":{"bsonType":"array","maxItems":2,"items":slot},
    });
    // Application validation additionally enforces exact cohort identifiers,
    // slot ownership and gate-specific authority before every control effect.
    doc! {"$jsonSchema":shape,"$or":[
        {"_id":"agent_log_segments_v1"},
        {"_id":"agent_log_capacity_recovery_v1","policy.owners":{"$size":0},
            "policy.basis_points":0,"policy.cohort_seed":"","slots.1":{"$exists":false}}
    ]}
}

pub(super) fn admission() -> Document {
    let shape = object(doc! {
        "_id":string(1800),"format_version":{"bsonType":"long","enum":[1_i64]},
        "agent_id":string(512),"incarnation":{"bsonType":"string","minLength":64,"maxLength":64},
        "owner":string(512),"publication_key":string(512),
        "fingerprint":{"bsonType":"binData"},"appended_at":string(64),
        "expected_agent_revision":long(0),
        "expected_seq":{"bsonType":"long","minimum":0_i64,"maximum":super::super::MAX_SEQUENCE},
        "admitted_at":string(64),"kind":{"enum":["activation","rollover","capacity_recovery"]},
        "writer_gate":proof("agent_log_segments_v1"),
        "recovery_gate":{"anyOf":[{"bsonType":"null"},proof("agent_log_capacity_recovery_v1")]},
    });
    doc! {"$jsonSchema":shape,"$or":[
        {"kind":"capacity_recovery","recovery_gate":{"$type":"object"}},
        {"kind":{"$in":["activation","rollover"]},"recovery_gate":{"$type":"null"}}
    ]}
}
