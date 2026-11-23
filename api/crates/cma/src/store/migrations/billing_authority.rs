use crate::store::billing_authority::format_v1;
use layer2_core::billing_authority::ACTIVATION_BLOCKERS;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ID: &str = "am-billing-authority-v1";

pub(super) fn command(arguments: &[String]) -> Option<super::Result<Value>> {
    if !arguments.iter().any(|argument| argument == ID) {
        return None;
    }
    Some(
        if arguments.len() == 2 && arguments[0] == "--migration-plan" && arguments[1] == ID {
            Ok(plan())
        } else {
            Err("billing authority foundation provides only --migration-plan; no apply or status capability")
        },
    )
}

fn plan() -> Value {
    let mut digest = Sha256::new();
    digest.update(ID);
    digest.update(include_bytes!("../billing_authority/format_v1.rs"));
    json!({
        "migration":ID,"checksum":format!("{:x}",digest.finalize()),"phase":"expand",
        "mode":"plan_only","writes":false,"apply_available":false,"activation_ready":false,
        "collection":"platform_settings","new_collections":[],"new_indexes":[],
        "proposed_documents":format_v1::proposed_documents(),
        "activation_blockers":ACTIVATION_BLOCKERS,
        "reader_compatibility":"both absent or supported prepared legacy epoch zero only",
        "future_preparation":"atomically install both records; permanently retain the reader floor",
        "startup":{"ddl":false,"backfill":false,"authority_writes":false},
        "legacy_obligations":"retain original funding, policy, period, partition and recovery identities",
        "fleet_readiness":"not established by reader compatibility or optional transport membership"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plan_is_offline_and_never_authorizes_preparation_or_activation() {
        let args = |mode: &str| vec![mode.to_owned(), ID.to_owned()];
        let value = crate::store::migrations::command(&args("--migration-plan"))
            .await
            .unwrap();
        assert_eq!(value["writes"], false);
        assert_eq!(value["apply_available"], false);
        assert_eq!(value["activation_ready"], false);
        assert_eq!(value["new_collections"], json!([]));
        assert_eq!(value["new_indexes"], json!([]));
        assert_eq!(value["proposed_documents"].as_array().unwrap().len(), 2);
        assert!(value["activation_blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "all_role_membership_unavailable"));
        for mode in [
            "--migration-apply",
            "--migration-status",
            "--migration-preflight",
            "--print-migration-role",
        ] {
            assert_eq!(crate::store::migrations::command(&args(mode)).await.unwrap_err(),
                "billing authority foundation provides only --migration-plan; no apply or status capability");
        }
    }
}
