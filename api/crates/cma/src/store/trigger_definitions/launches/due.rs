//! Shared discovery predicate for scheduling and durable telemetry.
use mongodb::bson::{doc, Bson, Document};

pub(in crate::store) fn filter(now: &str) -> Document {
    doc! {
        "work_step": { "$ne": "none" },
        "work_ready_at": { "$lte": now },
        "$and": [
            {
                "$or": [
                    { "blocked_reason": Bson::Null },
                    {
                        "blocked_reason.type": {
                            "$in": [
                                "connection_required",
                                "orchestration_paused",
                                "sandbox_paused_reviving",
                            ],
                        },
                    },
                ],
            },
            {
                "$or": [
                    { "work_lease_expires_at": Bson::Null },
                    { "work_lease_expires_at": { "$lte": now } },
                ],
            },
        ],
    }
}
