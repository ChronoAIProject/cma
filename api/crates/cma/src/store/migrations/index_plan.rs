//! Secret-safe projection of MongoDB explain plan topology.

use mongodb::bson::{Bson, Document};
use serde_json::{Map, Value};

pub(super) fn sanitize(plan: &Document, target_index: &str, safe_fields: &[&str]) -> Value {
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
            Value::String(if index_name == target_index {
                target_index.to_owned()
            } else {
                "<redacted>".to_owned()
            }),
        );
    }
    for key in ["keyPattern", "sortPattern"] {
        if let Ok(pattern) = plan.get_document(key) {
            output.insert(
                key.to_snake_case(),
                Value::Object(key_pattern(pattern, safe_fields)),
            );
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
            output.insert(
                key.to_snake_case(),
                sanitize(child, target_index, safe_fields),
            );
        } else if plan.contains_key(key) {
            output.insert("incomplete".into(), Value::Bool(true));
        }
    }
    for key in ["inputStages", "children"] {
        if let Ok(children) = plan.get_array(key) {
            let mut incomplete = false;
            let values = children
                .iter()
                .filter_map(|child| match child.as_document() {
                    Some(child) => Some(sanitize(child, target_index, safe_fields)),
                    None => {
                        incomplete = true;
                        None
                    }
                })
                .collect();
            output.insert(key.to_snake_case(), Value::Array(values));
            if incomplete {
                output.insert("incomplete".into(), Value::Bool(true));
            }
        }
    }
    if let Ok(shards) = plan.get_array("shards") {
        let mut incomplete = false;
        let values = shards
            .iter()
            .filter_map(|shard| match shard.as_document() {
                Some(shard) => Some(sanitize(shard, target_index, safe_fields)),
                None => {
                    incomplete = true;
                    None
                }
            })
            .collect();
        output.insert("shards".into(), Value::Array(values));
        if incomplete {
            output.insert("incomplete".into(), Value::Bool(true));
        }
    } else if let Ok(shards) = plan.get_document("shards") {
        let mut incomplete = false;
        let values = shards
            .values()
            .filter_map(|shard| match shard.as_document() {
                Some(shard) => Some(sanitize(shard, target_index, safe_fields)),
                None => {
                    incomplete = true;
                    None
                }
            })
            .collect();
        output.insert("shards".into(), Value::Array(values));
        if incomplete {
            output.insert("incomplete".into(), Value::Bool(true));
        }
    } else if plan.contains_key("shards") {
        output.insert("incomplete".into(), Value::Bool(true));
    }
    if output.is_empty() {
        output.insert("incomplete".into(), Value::Bool(true));
    }
    Value::Object(output)
}

/// A complete permitted topology must reach the expected index in every branch.
/// Unknown stages, missing children and empty shards cannot certify a clean plan.
pub(super) fn verifies(plan: &Value, expected: &str) -> bool {
    let Some(node) = plan.as_object() else {
        return false;
    };
    if node.get("incomplete") == Some(&Value::Bool(true)) {
        return false;
    }
    let stage = node.get("stage").and_then(Value::as_str);
    if matches!(stage, Some("IXSCAN" | "DISTINCT_SCAN" | "EXPRESS_IXSCAN")) {
        return node.get("index_name").and_then(Value::as_str) == Some(expected)
            && ![
                "input_stage",
                "outer_stage",
                "inner_stage",
                "query_plan",
                "winning_plan",
                "child",
                "input_stages",
                "children",
                "shards",
            ]
            .iter()
            .any(|key| node.contains_key(*key));
    }
    if stage.is_some_and(|s| {
        ![
            "LIMIT",
            "SKIP",
            "FETCH",
            "PROJECTION_SIMPLE",
            "PROJECTION_DEFAULT",
            "PROJECTION_COVERED",
            "SHARD_MERGE",
            "SINGLE_SHARD",
            "SHARDING_FILTER",
            "OR",
            "SORT_MERGE",
            "AND_HASH",
            "AND_SORTED",
        ]
        .contains(&s)
    }) {
        return false;
    }
    let mut children = Vec::new();
    for key in [
        "input_stage",
        "outer_stage",
        "inner_stage",
        "query_plan",
        "winning_plan",
        "child",
    ] {
        if let Some(child) = node.get(key) {
            children.push(child);
        }
    }
    for key in ["input_stages", "children", "shards"] {
        if let Some(value) = node.get(key) {
            let Some(array) = value.as_array() else {
                return false;
            };
            if array.is_empty() {
                return false;
            }
            children.extend(array);
        }
    }
    !children.is_empty() && children.into_iter().all(|child| verifies(child, expected))
}

fn key_pattern(pattern: &Document, safe_fields: &[&str]) -> Map<String, Value> {
    pattern
        .iter()
        .filter_map(|(key, value)| {
            if !safe_fields.contains(&key.as_str()) {
                return None;
            }
            let safe_key = key;
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

    fn sanitize(plan: &mongodb::bson::Document) -> serde_json::Value {
        super::sanitize(
            plan,
            "activity_recorded_at_id_desc",
            &["recorded_at", "_id"],
        )
    }

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
    fn explain_projection_keeps_array_shard_plans_and_marks_unsupported_shapes() {
        let plan = doc! {
            "stage":"SHARD_MERGE",
            "shards":[
                {"shardName":"shard-a","winningPlan":{
                    "stage":"IXSCAN",
                    "indexName":"activity_recorded_at_id_desc"
                }},
                {"shardName":"shard-b","connectionString":"mongodb://secret.example",
                 "winningPlan":{"stage":"SORT","private":"redact-me"}}
            ],
            "private":"redact-me"
        };
        let output = sanitize(&plan).to_string();
        assert!(output.contains("IXSCAN"));
        assert!(output.contains("SORT"));
        assert!(output.contains("activity_recorded_at_id_desc"));
        assert!(!output.contains("shard-a"));
        assert!(!output.contains("secret.example"));
        assert!(!output.contains("redact-me"));
        assert!(!output.contains("incomplete"));

        let incomplete = sanitize(&doc! {"stage":"SHARD_MERGE","shards":"unknown"});
        assert_eq!(incomplete["incomplete"], true);
    }

    #[test]
    fn unknown_index_names_are_redacted_but_plan_shape_remains() {
        let output = sanitize(&doc! {"stage":"IXSCAN","indexName":"customer-name"});
        assert_eq!(output["stage"], "IXSCAN");
        assert_eq!(output["index_name"], "<redacted>");
    }
}
