use super::*;
use layer2_core::resource_access::{ResourceOwner, ResourceTarget};
use mongodb::bson;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

const MAX_RECORD_BYTES: usize = 64 * 1024;

pub(super) fn encode<T: Serialize>(id: &str, record: &T) -> ExecutionResult<Document> {
    let value = serde_json::to_string(record).map_err(|_| ExecutionError::Corrupt)?;
    if value.len() > MAX_RECORD_BYTES {
        return Err(ExecutionError::Capacity);
    }
    Ok(doc! {"_id":id,"protocol":1_i32,"record":value})
}

pub(super) fn decode<T: DeserializeOwned>(document: &Document) -> ExecutionResult<T> {
    if !matches!(document.get_i32("protocol"), Ok(1)) {
        return Err(ExecutionError::Corrupt);
    }
    let text = document
        .get_str("record")
        .map_err(|_| ExecutionError::Corrupt)?;
    if text.len() > MAX_RECORD_BYTES {
        return Err(ExecutionError::Corrupt);
    }
    serde_json::from_str(text).map_err(|_| ExecutionError::Corrupt)
}

const ERASURE_KEYS: &str = "erasure_keys_v1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErasureKeys {
    sandbox_id: wire::OpaqueId,
    actor_subject: Option<wire::OpaqueId>,
    owner_subject: Option<wire::OpaqueId>,
    share_id: Option<wire::OpaqueId>,
}

impl ErasureKeys {
    fn check_basis(&self, basis: &wire::ExecutionBasisV1) -> ExecutionResult<()> {
        if self.share_id.as_ref() != basis.family().map(|family| &family.share_id) {
            return Err(ExecutionError::Corrupt);
        }
        use wire::ExecutionBasisV1::{Independent, IndependentOwner, Share};
        let owner = match basis {
            IndependentOwner { owner, .. } => {
                if self.actor_subject.is_none() || self.actor_subject != self.owner_subject {
                    return Err(ExecutionError::Corrupt);
                }
                owner
            }
            Independent {
                authority: wire::IndependentAuthorityV1::ExternalBinding { .. },
                owner,
                ..
            } => owner,
            _ if self.actor_subject.is_none() => return Err(ExecutionError::Corrupt),
            Share { .. } => return Ok(()),
            Independent { owner, .. } => owner,
        };
        if let Some(subject) = &self.owner_subject {
            let person = ResourceOwner::Person {
                subject: layer2_core::SubjectId::new(subject.as_str()),
            };
            if hash(&person)? != *owner {
                return Err(ExecutionError::Corrupt);
            }
        }
        Ok(())
    }
}

fn read_keys(document: &Document, mut expected: Document) -> ExecutionResult<Option<ErasureKeys>> {
    let mut keys = None;
    if let Some(value) = document.get(ERASURE_KEYS) {
        let fields = value.as_document().ok_or(ExecutionError::Corrupt)?;
        // Bound the four scalar fields before decoding the extension.
        if fields.len() > 4
            || fields.iter().any(|(name, value)| {
                name.len() > 13 || value.as_str().is_none_or(|text| text.len() > 128)
            })
        {
            return Err(ExecutionError::Corrupt);
        }
        keys = Some(
            bson::deserialize_from_document(fields.clone()).map_err(|_| ExecutionError::Corrupt)?,
        );
        expected.insert(ERASURE_KEYS, value.clone());
    }
    expected
        .eq(document)
        .then_some(keys)
        .ok_or(ExecutionError::Corrupt)
}

pub(super) fn grant_document(value: &GrantControlV1) -> ExecutionResult<Document> {
    value.validate()?;
    let mut document = encode(value.key.origin.as_str(), value)?;
    document.insert("revision", value.revision.get() as i64);
    Ok(document)
}
pub(super) fn read_grant(document: &Document) -> ExecutionResult<GrantControlV1> {
    let value: GrantControlV1 = decode(document)?;
    let keys = read_keys(document, grant_document(&value)?)?;
    if keys.is_some_and(|keys| {
        keys.actor_subject.is_some() || keys.share_id.as_ref() != Some(&value.key.share_id)
    }) {
        return Err(ExecutionError::Corrupt);
    }
    Ok(value)
}

pub(super) fn grant_receipt_id(origin: &wire::Digest) -> String {
    format!("grant_receipt:{}", origin.as_str())
}

pub(super) fn grant_receipt(control: &GrantControlV1) -> Document {
    doc! {"_id":grant_receipt_id(&control.key.origin), "protocol":1_i32,
    "origin":control.key.origin.as_str(), "share_id":control.key.share_id.as_str(),
    "epoch":control.epoch.get() as i64}
}

pub(super) fn grant_pair(
    origin: &wire::Digest,
    control: Option<&Document>,
    receipt: Option<&Document>,
) -> ExecutionResult<Option<GrantControlV1>> {
    match (control, receipt) {
        (None, None) => Ok(None),
        (Some(control), Some(receipt)) => {
            let value = read_grant(control)?;
            let mut expected = grant_receipt(&value);
            if let Some(keys) = control.get(ERASURE_KEYS) {
                expected.insert(ERASURE_KEYS, keys.clone());
            }
            if value.key.origin != *origin || expected != *receipt {
                return Err(ExecutionError::Corrupt);
            }
            Ok(Some(value))
        }
        _ => Err(ExecutionError::Corrupt),
    }
}

pub(super) fn member_document(value: &ExecutionRecordV1) -> ExecutionResult<Document> {
    value.validate()?;
    let mut document = encode(value.registration.as_str(), value)?;
    if let Some(family) = value.basis.family() {
        document.insert("family_origin", family.origin.as_str());
        document.insert("generation", family.generation.get() as i64);
    }
    document.insert("ordinal", i64::from(value.ordinal));
    Ok(document)
}
pub(super) fn read_member(document: &Document) -> ExecutionResult<ExecutionRecordV1> {
    let value = decode(document)?;
    if let Some(keys) = read_keys(document, member_document(&value)?)? {
        keys.check_basis(&value.basis)?;
        if matches!(&value.target, ResourceTarget::Sandbox { sandbox_id }
            if sandbox_id.as_str() != keys.sandbox_id.as_str())
        {
            return Err(ExecutionError::Corrupt);
        }
    }
    Ok(value)
}

pub(super) fn operation_document(value: &RevocationOperationV1) -> ExecutionResult<Document> {
    value.validate()?;
    let mut document = encode(value.key.operation_id.as_str(), value)?;
    document.insert(
        "phase",
        match value.phase {
            RevocationPhaseV1::Pending => "pending",
            RevocationPhaseV1::Effective { .. } => "effective",
        },
    );
    Ok(document)
}
pub(super) fn read_operation(document: &Document) -> ExecutionResult<RevocationOperationV1> {
    let value = decode(document)?;
    if let Some(keys) = read_keys(document, operation_document(&value)?)? {
        let intent = &value.intent;
        let actor = intent
            .actor
            .ordinary_subject()
            .map(|subject| subject.as_str());
        let owner = match &intent.owner {
            ResourceOwner::Person { subject } => Some(subject.as_str()),
            ResourceOwner::Organization { .. } => None,
        };
        if keys.share_id.as_ref() != Some(&value.key.family.share_id)
            || keys.sandbox_id.as_str() != intent.sandbox_id.as_str()
            || keys.owner_subject.as_ref().map(wire::OpaqueId::as_str) != owner
            || keys.actor_subject.as_ref().map(wire::OpaqueId::as_str) != actor
        {
            return Err(ExecutionError::Corrupt);
        }
    }
    Ok(value)
}

pub(super) fn hash(value: &impl Serialize) -> ExecutionResult<wire::Digest> {
    let bytes = serde_json::to_vec(value).map_err(|_| ExecutionError::Corrupt)?;
    wire::Digest::new(&format!("{:x}", Sha256::digest(bytes))).map_err(Into::into)
}
pub(super) fn effect_id(intent: &wire::EffectIntentV1) -> ExecutionResult<wire::Digest> {
    hash(&(
        "resource-effect-identity-v1",
        &intent.registration,
        &intent.effect_id,
    ))
}
pub(super) fn effect_document(value: &EffectRecordV1) -> ExecutionResult<Document> {
    value.validate()?;
    let mut document = encode(effect_id(&value.permission.intent)?.as_str(), value)?;
    document.insert(
        "registration",
        value.permission.intent.registration.as_str(),
    );
    document.insert("ordinal", value.permission.ledger_sequence.get() as i64);
    Ok(document)
}
pub(super) fn read_effect(document: &Document) -> ExecutionResult<EffectRecordV1> {
    let value = decode(document)?;
    if let Some(keys) = read_keys(document, effect_document(&value)?)? {
        keys.check_basis(&value.permission.intent.grant)?;
    }
    Ok(value)
}

impl MongoResourceExecutionStore {
    pub(super) async fn member_in(
        &self,
        registration: &wire::Digest,
        session: &mut ClientSession,
    ) -> TxnResult<ExecutionRecordV1> {
        let document = self
            .collections
            .members
            .find_one(doc! {"_id":registration.as_str()})
            .session(session)
            .await?
            .ok_or(ExecutionError::AuthorityChanged)?;
        Ok(read_member(&document)?)
    }
    pub(super) async fn save_member(
        &self,
        member: &ExecutionRecordV1,
        session: &mut ClientSession,
    ) -> TxnResult<()> {
        let result = self
            .collections
            .members
            .replace_one(
                doc! {"_id":member.registration.as_str()},
                member_document(member)?,
            )
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(ExecutionError::Corrupt.into());
        }
        Ok(())
    }
    pub(super) async fn operation_in(
        &self,
        id: &wire::Digest,
        session: &mut ClientSession,
    ) -> TxnResult<RevocationOperationV1> {
        let document = self
            .collections
            .operations
            .find_one(doc! {"_id":id.as_str()})
            .session(session)
            .await?
            .ok_or(ExecutionError::AuthorityChanged)?;
        Ok(read_operation(&document)?)
    }
    pub(super) async fn grant_in(
        &self,
        family: &wire::GrantFamilyKey,
        epoch: wire::Generation,
        session: &mut ClientSession,
    ) -> TxnResult<GrantControlV1> {
        match self.collections.grant_in(&family.origin, session).await? {
            Some(control) => {
                control.require_open(family, epoch)?;
                Ok(control)
            }
            None if family.generation.get() == 1 => Ok(GrantControlV1 {
                protocol: 1,
                key: family.clone(),
                epoch,
                revision: wire::Generation::new(1)?,
                members: 0,
                state: GrantStateV1::Open,
            }),
            None => Err(ExecutionError::Corrupt.into()),
        }
    }
    pub(super) async fn write_grant(
        &self,
        control: &GrantControlV1,
        session: &mut ClientSession,
    ) -> TxnResult<()> {
        if self
            .collections
            .grant_in(&control.key.origin, session)
            .await?
            .is_none()
        {
            // The immutable origin receipt distinguishes a new family from a
            // lost Closing/tombstone row, including a family with no members.
            self.collections
                .grants
                .insert_one(grant_receipt(control))
                .session(&mut *session)
                .await?;
        }
        self.collections
            .grants
            .replace_one(
                doc! {"_id":control.key.origin.as_str()},
                grant_document(control)?,
            )
            .upsert(true)
            .session(session)
            .await?;
        Ok(())
    }
}
