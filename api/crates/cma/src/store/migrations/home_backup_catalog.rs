//! Plan-only foundation: no production schema executor or migration ledger writer.
use crate::store::home_backups::schema_v1;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ID: &str = "am-home-backup-catalog-v1";

pub(super) fn command(arguments: &[String]) -> Option<super::Result<Value>> {
    if !arguments.iter().any(|argument| argument == ID) {
        return None;
    }
    Some(
        if arguments.len() == 2 && arguments[0] == "--migration-plan" && arguments[1] == ID {
            Ok(plan())
        } else {
            Err("home backup foundation provides only --migration-plan; no apply or status capability")
        },
    )
}

fn plan() -> Value {
    let mut digest = Sha256::new();
    digest.update(ID);
    digest.update(include_bytes!("home_backup_catalog.rs"));
    digest.update(include_bytes!("../home_backups/schema_v1.rs"));
    digest.update(include_bytes!("../schema/index_contract_v1.rs"));
    let collections: Vec<_> = schema_v1::COLLECTIONS
        .into_iter()
        .map(|name| json!({"name":name,"options":{},"indexes":schema_v1::indexes(name)}))
        .collect();
    json!({
        "migration":ID,"checksum":format!("{:x}",digest.finalize()),"phase":"expand",
        "mode":"plan_only","writes":false,"apply_available":false,
        "collections":collections,"document_schema_version":1,
        "prerequisites":["review compatible retained-record readers and assign recovery ownership",
            "separately authorize collection bootstrap and unique-key validation before any apply implementation"],
        "effects":"proposed additive collections and indexes only; no executable production step",
        "duplicate_policy":"refuse; never delete or rewrite records",
        "startup":{"ddl":false,"scan":false,"backfill":false},
        "retention_changes":false,"capture_available":false,
        "retained_work":{"status":"HOLD","inspection":"not_performed",
            "responsibility":"release owner must establish qualified recovery before capture activation"}
    })
}

#[cfg(test)]
pub(in crate::store) async fn prepare_for_test(database: &mongodb::Database) {
    for collection in schema_v1::COLLECTIONS {
        database.create_collection(collection).await.unwrap();
        database
            .run_command(mongodb::bson::doc! {
                "createIndexes":collection,"indexes":schema_v1::indexes(collection),
                "writeConcern":{"w":"majority","wtimeout":5000},
            })
            .await
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plan_is_offline_and_cannot_be_applied_or_report_capture_readiness() {
        let arguments = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).into())
                .collect::<Vec<_>>()
        };
        let result = crate::store::migrations::command(&arguments(&["--migration-plan", ID]))
            .await
            .unwrap();
        assert_eq!(result["collections"].as_array().unwrap().len(), 2);
        assert_eq!(result["writes"], false);
        assert_eq!(result["apply_available"], false);
        assert_eq!(result["capture_available"], false);
        assert_eq!(result["retained_work"]["status"], "HOLD");
        assert_eq!(result["retained_work"]["inspection"], "not_performed");
        for values in [
            vec!["--migration-apply", ID, "checksum", "image"],
            vec!["--migration-apply", ID, "image"],
            vec!["--migration-status", ID],
            vec!["--print-migration-role", ID],
            vec!["--migration-plan", ID, "extra"],
        ] {
            assert_eq!(crate::store::migrations::command(&arguments(&values)).await.unwrap_err(),
                "home backup foundation provides only --migration-plan; no apply or status capability");
        }
    }
}
