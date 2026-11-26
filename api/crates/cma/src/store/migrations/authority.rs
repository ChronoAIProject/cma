use cma_service_auth::mongodb_authority::{verify_shared_schema, MongoAuthorityMode};
use mongodb::bson::{doc, Document};

use super::{ledger::COLLECTION, steps::Step, Result};

#[cfg(test)]
pub(super) fn role(database: &str, apply: bool) -> Document {
    role_for(database, apply, Step::Activity)
}

pub(super) fn role_for(database: &str, apply: bool, step: Step) -> Document {
    let mut grants = vec![doc! {"resource":{"db":database,"collection":COLLECTION},
    "actions":if apply {vec!["find","insert","update","createCollection"]} else {vec!["find"]}}];
    for collection in step.collections() {
        let actions = if matches!(step, Step::BindingIndexes | Step::SandboxUsageCapture) {
            let mut actions = vec!["listIndexes", "find"];
            if apply {
                actions.push("createIndex");
                if step == Step::SandboxUsageCapture {
                    actions.push("createCollection");
                }
            }
            actions
        } else if !apply {
            vec!["listIndexes"]
        } else if matches!(
            step,
            Step::Activity
                | Step::AgentLogSegments
                | Step::MeasuredCustody
                | Step::UsageProjections
                | Step::MeasuredBatches
                | Step::MeasuredIndex
                | Step::AgentLogActiveProofs
        ) {
            vec!["listIndexes", "createIndex", "createCollection"]
        } else {
            vec!["listIndexes", "createIndex"]
        };
        grants.push(doc! {"resource":{"db":database,"collection":collection}, "actions":actions});
    }
    if matches!(
        step,
        Step::AgentLogSegments
            | Step::BindingIndexes
            | Step::SandboxUsageCapture
            | Step::MeasuredCustody
            | Step::UsageProjections
            | Step::MeasuredBatches
            | Step::MeasuredIndex
            | Step::AgentLogActiveProofs
    ) {
        grants
            .push(doc! {"resource":{"db":database,"collection":""},"actions":["listCollections"]});
    }
    doc! {"createRole":"cma_am_migrator","privileges":grants,"roles":[]}
}

#[cfg(test)]
pub(super) fn verify(
    status: &Document,
    database: &str,
    mode: MongoAuthorityMode,
    apply: bool,
) -> Result<()> {
    verify_for(status, database, mode, apply, Step::Activity)
}

pub(super) fn verify_for(
    status: &Document,
    database: &str,
    mode: MongoAuthorityMode,
    apply: bool,
    step: Step,
) -> Result<()> {
    verify_shared_schema(status, database, &role_for(database, apply, step))
        .map_err(|_| "migration database authority is insufficient")?;
    if mode == MongoAuthorityMode::Isolated {
        let info = status
            .get_document("authInfo")
            .map_err(|_| "migration authority malformed")?;
        let permitted = role_for(database, true, step);
        for grant in info
            .get_array("authenticatedUserPrivileges")
            .map_err(|_| "migration grants malformed")?
        {
            let grant = grant.as_document().ok_or("migration grant malformed")?;
            let resource = grant
                .get_document("resource")
                .map_err(|_| "migration resource malformed")?;
            let expected = permitted
                .get_array("privileges")
                .unwrap()
                .iter()
                .filter_map(|value| value.as_document())
                .find(|value| value.get_document("resource").ok() == Some(resource))
                .ok_or("migration credential is not service isolated")?;
            let allowed = expected.get_array("actions").unwrap();
            if grant
                .get_array("actions")
                .map_err(|_| "migration actions malformed")?
                .iter()
                .any(|action| !allowed.contains(action))
            {
                return Err("migration credential has unreviewed authority");
            }
        }
    }
    Ok(())
}
