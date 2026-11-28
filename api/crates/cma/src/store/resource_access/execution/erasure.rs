//! Attribution copied from an authorized admission, before the wire record is hashed.
use super::*;
use layer2_core::resource_access::ResourceOwner;
use layer2_core::SubjectId;

pub(in crate::store) use super::records::ERASURE_KEYS;
pub(in crate::store) const SANDBOX: &str = "erasure_keys_v1.sandbox_id";
pub(in crate::store) const OWNER: &str = "erasure_keys_v1.owner_subject";
pub(in crate::store) const ACTOR: &str = "erasure_keys_v1.actor_subject";

pub(super) fn keys(
    sandbox: &str,
    owner: &ResourceOwner,
    actor: Option<&SubjectId>,
    share: Option<&wire::OpaqueId>,
) -> ExecutionResult<Document> {
    let mut keys = doc! {"sandbox_id": wire::OpaqueId::new(sandbox)?.as_str()};
    if let ResourceOwner::Person { subject } = owner {
        keys.insert(
            "owner_subject",
            wire::OpaqueId::new(subject.as_str())?.as_str(),
        );
    }
    if let Some(subject) = actor {
        keys.insert(
            "actor_subject",
            wire::OpaqueId::new(subject.as_str())?.as_str(),
        );
    }
    if let Some(share) = share {
        keys.insert("share_id", share.as_str());
    }
    Ok(keys)
}

pub(super) fn admission(authority: &ExecutionAuthority, actor: bool) -> ExecutionResult<Document> {
    let snapshot = authority.snapshot();
    let scope = snapshot
        .scope(authority.target())
        .map_err(ExecutionError::from)?;
    keys(
        scope.sandbox_id.as_str(),
        &scope.owner,
        actor.then(|| authority.erasure_subject()),
        authority.basis().family().map(|family| &family.share_id),
    )
}

pub(in crate::store) const SHARE: &str = "erasure_keys_v1.share_id";
pub(in crate::store) use super::collections::{EFFECTS, GRANTS, MEMBERS, OPERATIONS};
pub(in crate::store) const COLLECTIONS: &[&str] = &[EFFECTS, GRANTS, MEMBERS, OPERATIONS];

impl MongoResourceExecutionStore {
    pub(super) async fn writes_erasure_keys(
        &self,
        family: Option<&wire::GrantFamilyKey>,
        session: &mut ClientSession,
    ) -> TxnResult<bool> {
        if self.write_erasure_keys {
            return Ok(true);
        }
        let Some(family) = family else {
            return Ok(false);
        };
        // A keyed family must stay fully attributable when new key admission
        // is switched off. Existing legacy families stay readable as before.
        Ok(self
            .collections
            .grants
            .find_one(doc! {
                "_id": family.origin.as_str(),
                ERASURE_KEYS: {"$exists": true},
            })
            .projection(doc! {"_id":1})
            .session(session)
            .await?
            .is_some())
    }
}

mod remove;
pub(in crate::store) use remove::{erase, EraseError, Scope};
