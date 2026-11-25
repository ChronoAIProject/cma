//! Explicit additive admission-cohort access path; never installed by serving code.
use super::{index_inventory, Result};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

pub(super) const ID: &str = "am-command-cohort-index-v1";
pub(super) fn checksum() -> String {
    format!(
        "{:x}",
        Sha256::digest(include_bytes!("command_cohort_index.rs"))
    )
}
pub(in crate::store) fn index() -> Document {
    doc! {"name":"agent_command_admission_cohort","key":{"record.receipt.admitted_at":1,"_id":1}}
}
pub(super) async fn inspect(database: &Database) -> Result<Vec<Document>> {
    index_inventory::strict_pending(database, "agent_commands", &index()).await
}
pub(super) async fn create(database: &Database, requested: Document) -> Result<bool> {
    if requested.len() != 2
        || !index_inventory::strict_missing(std::slice::from_ref(&requested), &index())?.is_empty()
    {
        return Err("unreviewed Command cohort index request");
    }
    let response = database
        .run_command(
            doc! {"createIndexes":"agent_commands","indexes":[requested],"maxTimeMS":10000,
            "writeConcern":{"w":"majority","wtimeout":5000}},
        )
        .await
        .map_err(|_| "Command cohort index result uncertain; reconcile before retry")?;
    if response.contains_key("writeConcernError") {
        return Err("Command cohort index acknowledgement uncertain");
    }
    let count = |key| {
        response
            .get_i64(key)
            .or_else(|_| response.get_i32(key).map(i64::from))
            .map_err(|_| "Command cohort index count malformed")
    };
    let before = count("numIndexesBefore")?;
    let after = count("numIndexesAfter")?;
    if before < 0 || after < before || before.checked_add(1).is_none_or(|max| after > max) {
        return Err("Command cohort index count inconsistent");
    }
    Ok(after > before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::migrations::steps::Step;

    #[test]
    fn cohort_index_has_an_explicit_manifest_and_refuses_semantic_drift() {
        let step = Step::parse(ID).unwrap();
        assert_eq!(step.collections(), &["agent_commands"]);
        assert_eq!(step.max_changes(), 1);
        assert_eq!(step.checksum(), checksum());
        assert_eq!(checksum().len(), 64);
        let desired = index();
        assert!(
            index_inventory::strict_missing(std::slice::from_ref(&desired), &desired)
                .unwrap()
                .is_empty()
        );
        for field in [
            "unique",
            "sparse",
            "hidden",
            "expireAfterSeconds",
            "collation",
        ] {
            let mut drift = desired.clone();
            drift.insert(field, true);
            assert!(index_inventory::strict_missing(&[drift], &desired).is_err());
        }
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
    async fn cohort_index_creation_is_explicit_additive_and_resumable() {
        let client =
            mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
                .await
                .unwrap();
        let database = client.database(&format!(
            "cma_cohort_index_{}",
            uuid::Uuid::new_v4().simple()
        ));
        database.create_collection("agent_commands").await.unwrap();
        assert_eq!(inspect(&database).await.unwrap(), vec![index()]);
        assert!(create(&database, index()).await.unwrap());
        assert!(!create(&database, index()).await.unwrap());
        assert!(inspect(&database).await.unwrap().is_empty());
        let mut drift = index();
        drift.insert("unique", true);
        assert!(create(&database, drift).await.is_err());
        database.drop().await.unwrap();
    }
}
