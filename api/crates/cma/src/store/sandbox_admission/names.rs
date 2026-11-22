use super::{AdmissionError, MongoSandboxStore};
use layer2_core::sandbox::store::{
    allocated_sandbox_name, SandboxAdmissionName, SandboxStoreError,
};
use mongodb::bson::{doc, Bson};

pub(super) async fn choose_name(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
    owner: &str,
    requested: &SandboxAdmissionName,
) -> Result<String, AdmissionError> {
    match requested {
        SandboxAdmissionName::Exact(name) => {
            if name_exists(store, session, owner, name).await? {
                Err(AdmissionError::Domain(SandboxStoreError::Duplicate))
            } else {
                Ok(name.clone())
            }
        }
        SandboxAdmissionName::Allocate(base) => {
            for number in 1_u64.. {
                let candidate = suffixed(base, number);
                if !name_exists(store, session, owner, &candidate).await? {
                    return Ok(candidate);
                }
            }
            unreachable!("the numeric sandbox-name suffix space is unbounded")
        }
    }
}

async fn name_exists(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
    owner: &str,
    name: &str,
) -> Result<bool, AdmissionError> {
    store
        .sandboxes
        .find_one(doc! { "owner": owner, "name": name, "retired_at": Bson::Null })
        .projection(doc! { "_id": 1 })
        .session(session)
        .await
        .map(|row| row.is_some())
        .map_err(AdmissionError::Driver)
}

pub(super) fn suffixed(base: &str, number: u64) -> String {
    allocated_sandbox_name(base, number).expect("admission names are validated before allocation")
}
