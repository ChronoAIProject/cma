//! Exact historical automation validators.
use mongodb::bson::{doc, Document};

pub(super) fn prior_validators() -> Vec<Document> {
    vec![
        // 38f71b1743c
        {
            let mut validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","kind","version","schema_version","blob"],"properties":{"_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"kind":{"enum":["binding","binding_operation","binding_approval","delivery","execution_group","app_integration","app_registration","webhook_credential","app_operation","app_identity"]},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]},"guard_revision":{"bsonType":"long","minimum":0},"lease_owner":{"bsonType":"string"},"lease_until":{"bsonType":"long"},"expires_at":{"bsonType":"date"},"blob":{"bsonType":"binData"}}}};
            validator.insert(
                "$or",
                mongodb::bson::bson!([
                    {"kind":"delivery","expires_at":{"$type":"date"}},
                    {"kind":{"$ne":"delivery"},"expires_at":{"$exists":false}}
                ]),
            );
            validator
        },
        // d41a39247c6
        {
            let mut validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","kind","version","schema_version","blob"],"properties":{"_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"kind":{"enum":["binding","binding_operation","binding_approval","delivery","execution_group","app_integration","app_registration","webhook_credential","app_operation","app_identity"]},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]},"guard_revision":{"bsonType":"long","minimum":0},"expires_at":{"bsonType":"date"},"blob":{"bsonType":"binData"}}}};
            validator.insert(
                "$or",
                mongodb::bson::bson!([
                    {"kind":"delivery","expires_at":{"$type":"date"}},
                    {"kind":{"$ne":"delivery"},"expires_at":{"$exists":false}}
                ]),
            );
            validator
        },
        // 0a1105e56d7
        doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","kind","version","schema_version","blob"],"properties":{"_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"kind":{"enum":["binding","binding_operation","binding_approval","delivery","execution_group","app_integration","app_registration","webhook_credential","app_operation","app_identity"]},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]},"guard_revision":{"bsonType":"long","minimum":0},"blob":{"bsonType":"binData"}}}},
        // 0a987cd73fa
        doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","kind","version","schema_version","blob"],"properties":{"_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"kind":{"enum":["binding","delivery","execution_group","app_integration","app_registration","webhook_credential","app_operation","app_identity"]},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]},"blob":{"bsonType":"binData"}}}},
    ]
}
