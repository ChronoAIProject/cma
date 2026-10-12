//! Persist explicit ownership in the same transaction as sandbox admission.

use layer2_core::{resource_access::ResourcePolicyRecord, sandbox::store::SandboxStoreError};
use mongodb::ClientSession;

use super::super::{
    sandbox_admission::AdmissionError,
    sandbox_mongo::{is_duplicate_key, MongoSandboxStore},
};

pub(in crate::store) async fn insert_admission_policy(
    store: &MongoSandboxStore,
    policy: Option<&ResourcePolicyRecord>,
    session: &mut ClientSession,
) -> Result<(), AdmissionError> {
    let Some(policy) = policy else {
        return Ok(());
    };
    let document = super::records::policy_document(policy).map_err(|_| {
        AdmissionError::Domain(SandboxStoreError::Corrupt(
            "invalid organization ownership".into(),
        ))
    })?;
    store
        .resource_policies
        .insert_one(document)
        .session(session)
        .await
        .map_err(|error| {
            if is_duplicate_key(&error) {
                AdmissionError::Duplicate
            } else {
                AdmissionError::Driver(error)
            }
        })?;
    Ok(())
}
