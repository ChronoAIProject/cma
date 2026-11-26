//! Historical marker validators from the corresponding schema revisions.
use mongodb::bson::{doc, Document};

pub(super) fn marker_predecessors() -> Vec<Document> {
    vec![
        // v42, before the dedicated Channel creation writer floor.
        doc! {
            "$jsonSchema":{"bsonType":"object","additionalProperties":false,
                "required":["_id","version"],"properties":{
                    "_id":{"bsonType":"string"},"version":{"enum":[1,2,3,4,5,6]}}},
            "$or":[{"_id":"schema","version":2},{"_id":"replicas","version":1},
                {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1},
                {"_id":"media_continuations","version":1},{"_id":"worker_credentials","version":1},
                {"_id":"worker_tracking_cutover","version":1},
                {"_id":"github_channel_guard","version":1},
                {"_id":"github_integrations","version":{"$in":[1,2,3,4,5,6]}}]
        },
        // 803bc792508
        doc! {
            "$jsonSchema":{"bsonType":"object","additionalProperties":false,
                "required":["_id","version"],"properties":{
                    "_id":{"bsonType":"string"},"version":{"enum":[1,2,3,4,5,6]}}},
            "$or":[{"_id":"schema","version":2},
                {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1},
                {"_id":"media_continuations","version":1},{"_id":"worker_credentials","version":1},
                {"_id":"worker_tracking_cutover","version":1},
                {"_id":"github_channel_guard","version":1},
                {"_id":"github_integrations","version":{"$in":[1,2,3,4,5,6]}}]
        },
        // 38f71b1743c
        doc! {
            "$jsonSchema":{"bsonType":"object","additionalProperties":false,
                "required":["_id","version"],"properties":{
                    "_id":{"bsonType":"string"},"version":{"enum":[1,2,3,4]}}},
            "$or":[{"_id":"schema","version":2},
                {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1},
                {"_id":"media_continuations","version":1},{"_id":"worker_credentials","version":1},
                {"_id":"worker_tracking_cutover","version":1},
                {"_id":"github_integrations","version":{"$in":[1,2,3,4]}}]
        },
        // 0a1105e56d7
        doc! {
            "$jsonSchema":{"bsonType":"object","additionalProperties":false,
                "required":["_id","version"],"properties":{
                    "_id":{"bsonType":"string"},"version":{"enum":[1,2,3]}}},
            "$or":[{"_id":"schema","version":2},
                {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1},
                {"_id":"media_continuations","version":1},{"_id":"worker_credentials","version":1},
                {"_id":"worker_tracking_cutover","version":1},
                {"_id":"github_integrations","version":{"$in":[1,2,3]}}]
        },
        // 0a987cd73fa
        doc! {
            "$jsonSchema":{"bsonType":"object","additionalProperties":false,
                "required":["_id","version"],"properties":{
                    "_id":{"bsonType":"string"},"version":{"enum":[1,2]}}},
            "$or":[{"_id":"schema","version":2},
                {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1},
                {"_id":"media_continuations","version":1},{"_id":"worker_credentials","version":1},
                {"_id":"worker_tracking_cutover","version":1},
                {"_id":"github_integrations","version":{"$in":[1,2]}}]
        },
        // ef5835678b3
        doc! {
            "$jsonSchema":{"bsonType":"object","additionalProperties":false,
                "required":["_id","version"],"properties":{
                    "_id":{"bsonType":"string"},"version":{"enum":[1,2]}}},
            "$or":[{"_id":"schema","version":2},
                {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1},
                {"_id":"media_continuations","version":1},{"_id":"worker_credentials","version":1},
                {"_id":"worker_tracking_cutover","version":1}]
        },
        // 4c05173e41b
        doc! {
            "$jsonSchema":{"bsonType":"object","additionalProperties":false,
                "required":["_id","version"],"properties":{
                    "_id":{"bsonType":"string"},"version":{"enum":[1,2]}}},
            "$or":[{"_id":"schema","version":2},
                {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1},
                {"_id":"media_continuations","version":1}]
        },
        // 1741b1e77fa
        doc! {
            "$jsonSchema":{"bsonType":"object","additionalProperties":false,
                "required":["_id","version"],"properties":{
                    "_id":{"bsonType":"string"},"version":{"enum":[1,2]}}},
            "$or":[{"_id":"schema","version":2},
                {"_id":"thread_fences","version":1},{"_id":"whatsapp_forms","version":1}]
        },
        // 992aff34d28
        doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","version"],"properties":{"_id":{"enum":["schema","whatsapp_forms"]},"version":{"enum":[1,2]}}}},
        // de5e49bbb16
        doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","version"],"properties":{"_id":{"enum":["schema"]},"version":{"enum":[2]}}}},
    ]
}

pub(in crate::records) fn record_predecessors(current: &Document) -> Vec<Document> {
    let mut without_replica = current.clone();
    without_replica
        .get_document_mut("$jsonSchema")
        .unwrap()
        .get_document_mut("properties")
        .unwrap()
        .remove("replica");
    let mut without_replica_or_guard = without_replica.clone();
    without_replica_or_guard
        .get_document_mut("$jsonSchema")
        .unwrap()
        .get_document_mut("properties")
        .unwrap()
        .remove("guard_revision");
    let mut without_guard = current.clone();
    without_guard
        .get_document_mut("$jsonSchema")
        .unwrap()
        .get_document_mut("properties")
        .unwrap()
        .remove("guard_revision");
    vec![without_replica, without_replica_or_guard, without_guard]
}
