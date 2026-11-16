//! Analyst candidates are inspectable without granting a new migration path.
use super::catalog::{Binding, Query};
use mongodb::bson::{doc, Document};
use std::time::SystemTime;

fn candidate(
    name: &'static str,
    collection: &'static str,
    index: Document,
    filter: Document,
    sort: Document,
    qualification: &'static str,
) -> Query {
    Query {
        name,
        collection,
        index,
        filter,
        sort,
        qualification,
        binding: Binding::Static,
        projection: doc! {},
        limit: 51,
        step: None,
    }
}

pub(super) fn usage() -> Vec<Query> {
    let mut interval = candidate(
        "usage-event-interval",
        "sandbox_usage_events",
        doc! {"name":"sandbox_usage_events_interval",
            "key":{"sandbox_id":1,"ended_at":1,"started_at":1,"revision":1}},
        doc! {},
        doc! {},
        "startup interval index; one sampled sandbox/UTC day, first 51 events only; not complete allocation or full report evidence",
    );
    interval.binding = Binding::UsageInterval;
    let at = mongodb::bson::DateTime::now().timestamp_millis() / 1000;
    let filter = doc! {"reserved":true,"$or":[
        {"logical_expires_at":{"$lte":at}},
        {"kind":"retained_view_v1","retire_after":{"$lte":at}}
    ]};
    let prune = |name, index| {
        let mut query = candidate(
            name, "sandbox_usage_reports", index, filter.clone(), doc! {},
            "complete prune OR predicate at inspection time; both branches need access paths; two partial indexes add reservation writes",
        );
        query.limit = 8;
        query
    };
    vec![
        interval,
        prune(
            "usage-reserved-expiry",
            doc! {"name":"usage_reports_reserved_expiry",
            "key":{"logical_expires_at":1},"partialFilterExpression":{"reserved":true}},
        ),
        prune(
            "usage-reserved-retire",
            doc! {"name":"usage_reports_reserved_retire",
            "key":{"kind":1,"retire_after":1},"partialFilterExpression":{"reserved":true}},
        ),
    ]
}

pub(super) fn claims() -> Vec<Query> {
    let now = layer2_core::rfc3339_at(SystemTime::now());
    let worker = |name, collection, index_name, sandbox| {
        let mut filter = doc! {
            "document_schema_version":crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
            "work_pending":true,"work_ready_at":{"$lte":now.clone()},
            "$or":[{"work_lease_expires_at":null},{"work_lease_expires_at":{"$lte":now.clone()}}]
        };
        if sandbox {
            filter.insert(
                "$nor",
                vec![doc! {"record.lifecycle.retirement_archive.verified":{"$type":"null"}}],
            );
        }
        let mut query = candidate(name, collection,
            doc! {"name":index_name,"key":{"work_ready_at":1,"_id":1,"work_lease_expires_at":1},
                "partialFilterExpression":{"work_pending":true}},
            filter, doc! {"work_ready_at":1,"_id":1},
            "due-work discovery at inspection time, limit 51; no claim/update; candidate adds lease-write work",
        );
        query.projection = doc! {"_id":1};
        query
    };
    let mut native = candidate(
        "native-ready-agent-claim", "native_prepared_sandboxes",
        doc! {"name":"native_prepared_ready_page_candidate","key":{"kind":1,"state":1,"_id":1}},
        doc! {"kind":"reservation","state":"ready","machine":{"$ne":true}},
        doc! {"_id":1},
        "agent-only ready candidate discovery, no claim/update; small bounded pool; machine channel not measured",
    );
    native.limit = 2;
    vec![
        worker(
            "sandboxes-pending-claim",
            "sandboxes",
            "sandboxes_pending_ready_page",
            true,
        ),
        worker(
            "workspaces-pending-claim",
            "workspaces",
            "workspaces_pending_ready_page",
            false,
        ),
        native,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidates_preserve_or_branches_sort_and_time_types() {
        let usage = usage();
        assert!(usage[0].sort.is_empty());
        assert!(matches!(usage[0].binding, Binding::UsageInterval));
        assert_eq!(usage[0].collection, "sandbox_usage_events");
        assert_eq!(usage[0].limit, 51);
        assert!(usage[0]
            .index
            .get_document("key")
            .unwrap()
            .iter()
            .eq(doc! {"sandbox_id":1,"ended_at":1,"started_at":1,"revision":1}.iter()));
        assert_eq!(usage[1].filter, usage[2].filter);
        let branches = usage[1].filter.get_array("$or").unwrap();
        assert_eq!(branches.len(), 2);
        assert!(branches[0]
            .as_document()
            .unwrap()
            .get_document("logical_expires_at")
            .unwrap()
            .get_i64("$lte")
            .is_ok());
        let claims = claims();
        assert!(claims[0].filter.contains_key("$nor"));
        assert!(!claims[1].filter.contains_key("$nor"));
        assert!(layer2_core::seconds_from_rfc3339(
            claims[0]
                .filter
                .get_document("work_ready_at")
                .unwrap()
                .get_str("$lte")
                .unwrap()
        )
        .is_some());
        assert_eq!(
            claims[2].filter.get_document("machine").unwrap(),
            &doc! {"$ne":true}
        );
        for query in usage.iter().chain(&claims) {
            assert!(query.migration().is_err());
            assert!(crate::store::isolation::COLLECTIONS.contains(&query.collection));
        }
    }
}
