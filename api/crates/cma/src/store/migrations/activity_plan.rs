//! Secret-safe projection of MongoDB explain plan topology.

use mongodb::bson::{Bson, Document};
use serde_json::{Map, Value};

const TARGET_INDEX: &str = "activity_recorded_at_id_desc";

pub(super) fn sanitize(plan: &Document) -> Value {
    let mut output = Map::new();
    if let Some(stage) = plan
        .get_str("stage")
        .ok()
        .or_else(|| plan.get_str("nodeType").ok())
    {
        output.insert("stage".into(), Value::String(label(stage)));
    }
    if let Some(node_id) = integer(plan, "planNodeId") {
        output.insert("plan_node_id".into(), node_id.into());
    }
    if let Ok(index_name) = plan.get_str("indexName") {
        output.insert(
            "index_name".into(),
            Value::String(if index_name == TARGET_INDEX {
                TARGET_INDEX.to_owned()
            } else {
                "<redacted>".to_owned()
            }),
        );
    }
    for key in ["keyPattern", "sortPattern"] {
        if let Ok(pattern) = plan.get_document(key) {
            output.insert(key.to_snake_case(), Value::Object(key_pattern(pattern)));
        }
    }
    if let Ok(direction) = plan.get_str("direction") {
        output.insert("direction".into(), Value::String(label(direction)));
    }
    for key in [
        "inputStage",
        "outerStage",
        "innerStage",
        "queryPlan",
        "winningPlan",
        "child",
    ] {
        if let Ok(child) = plan.get_document(key) {
            output.insert(key.to_snake_case(), sanitize(child));
        }
    }
    for key in ["inputStages", "children"] {
        if let Ok(children) = plan.get_array(key) {
            let values = children
                .iter()
                .filter_map(Bson::as_document)
                .map(sanitize)
                .collect();
            output.insert(key.to_snake_case(), Value::Array(values));
        }
    }
    if let Ok(shards) = plan.get_document("shards") {
        let values = shards
            .values()
            .filter_map(Bson::as_document)
            .map(sanitize)
            .collect();
        output.insert("shards".into(), Value::Array(values));
    }
    Value::Object(output)
}

fn key_pattern(pattern: &Document) -> Map<String, Value> {
    pattern
        .iter()
        .filter_map(|(key, value)| {
            let safe_key = match key.as_str() {
                "recorded_at" | "_id" => key,
                _ => return None,
            };
            let direction = match value {
                Bson::Int32(value) if *value == 1 || *value == -1 => i64::from(*value),
                Bson::Int64(value) if *value == 1 || *value == -1 => *value,
                _ => return None,
            };
            Some((safe_key.to_owned(), direction.into()))
        })
        .collect()
}

fn integer(document: &Document, key: &str) -> Option<i64> {
    document
        .get_i64(key)
        .ok()
        .or_else(|| document.get_i32(key).ok().map(i64::from))
}

fn label(value: &str) -> String {
    if value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        value.to_owned()
    } else {
        "<redacted>".into()
    }
}

trait SnakeCase {
    fn to_snake_case(&self) -> String;
}

impl SnakeCase for str {
    fn to_snake_case(&self) -> String {
        self.chars()
            .map(|character| {
                if character.is_ascii_uppercase() {
                    format!("_{}", character.to_ascii_lowercase())
                } else {
                    character.to_string()
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use mongodb::bson::doc;

    use super::sanitize;

    #[test]
    fn explain_projection_omits_shard_identifiers_and_unknown_fields() {
        let plan = doc! {
            "stage":"SHARD_MERGE",
            "connectionString":"mongodb://secret.example",
            "shards": {
                "shard-a": {"winningPlan": {
                    "stage":"IXSCAN",
                    "indexName":"activity_recorded_at_id_desc",
                    "keyPattern":{"recorded_at":-1,"_id":-1,"private":1},
                    "slotBasedPlan":"secret plan text"
                }}
            }
        };
        let output = sanitize(&plan).to_string();
        assert!(output.contains("SHARD_MERGE"));
        assert!(output.contains("activity_recorded_at_id_desc"));
        assert!(!output.contains("shard-a"));
        assert!(!output.contains("secret.example"));
        assert!(!output.contains("secret plan text"));
        assert!(!output.contains("private"));
    }

    #[test]
    fn unknown_index_names_are_redacted_but_plan_shape_remains() {
        let output = sanitize(&doc! {"stage":"IXSCAN","indexName":"customer-name"});
        assert_eq!(output["stage"], "IXSCAN");
        assert_eq!(output["index_name"], "<redacted>");
    }
}
