use super::*;
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

pub(super) fn grant_document(value: &GrantControlV1) -> ExecutionResult<Document> {
    value.validate()?;
    let mut document = encode(value.key.origin.as_str(), value)?;
    document.insert("revision", value.revision.get() as i64);
    Ok(document)
}
pub(super) fn read_grant(document: &Document) -> ExecutionResult<GrantControlV1> {
    let value = decode(document)?;
    if grant_document(&value)? != *document {
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
            let control = read_grant(control)?;
            if control.key.origin != *origin || grant_receipt(&control) != *receipt {
                return Err(ExecutionError::Corrupt);
            }
            Ok(Some(control))
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
    if member_document(&value)? != *document {
        return Err(ExecutionError::Corrupt);
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
    if operation_document(&value)? != *document {
        return Err(ExecutionError::Corrupt);
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
    if effect_document(&value)? != *document {
        return Err(ExecutionError::Corrupt);
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
