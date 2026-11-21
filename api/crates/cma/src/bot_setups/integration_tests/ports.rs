use super::*;
use crate::sandbox::{
    CredentialCustodyError, CredentialMaterial, CredentialRevocation, SandboxCredentialCustody,
};
use layer2_core::{ActivityQuery, ActivityRecord, ActivityStore, Page, SandboxId, StoreError};

#[derive(Default)]
pub(super) struct Contexts(Mutex<std::collections::HashMap<String, Value>>);
#[async_trait::async_trait]
impl authority::ContextStore for Contexts {
    async fn retain(
        &self,
        binding: &str,
        chat: &str,
        event: &str,
        _: i64,
        value: &Value,
    ) -> Result<(), BotError> {
        self.0
            .lock()
            .unwrap()
            .insert(format!("{binding}/{chat}/{event}"), value.clone());
        Ok(())
    }
    async fn get(&self, binding: &str, chat: &str, event: &str) -> Result<Option<Value>, BotError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&format!("{binding}/{chat}/{event}"))
            .cloned())
    }
}

#[derive(Default)]
pub(super) struct Audit(pub Mutex<Vec<ActivityRecord>>);
#[async_trait::async_trait]
impl ActivityStore for Audit {
    async fn append(&self, value: &ActivityRecord) -> Result<(), StoreError> {
        self.0.lock().unwrap().push(value.clone());
        Ok(())
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        unreachable!()
    }
}

pub(super) struct Custody(pub CredentialMaterial);
#[async_trait::async_trait]
impl SandboxCredentialCustody for Custody {
    async fn list(
        &self,
        id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialCustodyError> {
        Ok(if id == &self.0.sandbox_id {
            vec![self.0.clone()]
        } else {
            vec![]
        })
    }
    async fn insert(&self, _: CredentialMaterial) -> Result<(), CredentialCustodyError> {
        unreachable!()
    }
    async fn get(
        &self,
        _: &SandboxId,
        _: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialCustodyError> {
        unreachable!()
    }
    async fn delete(&self, _: &SandboxId, _: u64) -> Result<(), CredentialCustodyError> {
        unreachable!()
    }
    async fn queue_revocation(
        &self,
        _: CredentialRevocation,
    ) -> Result<(), CredentialCustodyError> {
        unreachable!()
    }
    async fn revocations(
        &self,
        _: &SandboxId,
    ) -> Result<Vec<CredentialRevocation>, CredentialCustodyError> {
        unreachable!()
    }
    async fn delete_revocation(
        &self,
        _: &SandboxId,
        _: &str,
    ) -> Result<(), CredentialCustodyError> {
        unreachable!()
    }
}
