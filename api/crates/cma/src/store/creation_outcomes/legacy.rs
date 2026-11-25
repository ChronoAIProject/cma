use super::*;
use contract::{Algorithm, UnknownReason};
use futures_util::TryStreamExt;
use mongodb::bson::Bson;
use serde::Deserialize;

use reads::{unknown, valid_sandbox_id};

const MAX_RECEIPT_BYTES: i64 = 65_536;

impl CreationOutcomes {
    pub(super) async fn legacy(
        &self,
        snapshot: &Database,
        identity: &Identity<'_>,
        query: &LookupRequest,
        session: &mut ClientSession,
    ) -> Result<Outcome, StatusCode> {
        match identity.operation {
            Operation::Sandbox if self.config.legacy_custody => {
                Self::legacy_claim(snapshot, identity, query, session).await
            }
            Operation::Sandbox => Ok(unknown(UnknownReason::CoverageUnavailable)),
            Operation::AgentBootstrap => {
                Self::legacy_lifecycle(snapshot, identity, query, session).await
            }
        }
    }

    async fn legacy_claim(
        snapshot: &Database,
        identity: &Identity<'_>,
        query: &LookupRequest,
        session: &mut ClientSession,
    ) -> Result<Outcome, StatusCode> {
        let Some(row) = Self::read_claim(snapshot, identity, session).await? else {
            return Ok(unknown(UnknownReason::NoDurableEvidence));
        };
        if identity.algorithm != Algorithm::Sha256Wire {
            return Ok(unknown(UnknownReason::LegacyFingerprintUnavailable));
        }
        if query.claim_id.is_none() {
            return Ok(unknown(UnknownReason::GenerationAmbiguous));
        }
        let Some(claim) = exact_claim(&row, query.claim_id.as_deref())? else {
            return Ok(unknown(UnknownReason::GenerationAmbiguous));
        };
        check_fingerprint(&row, identity.wire_digest)?;
        if row.get_bool("oversize").map_err(unavailable)? {
            return Ok(unknown(UnknownReason::RecoveryRequired));
        }
        if let Some(bytes) = receipt(&row)? {
            #[derive(Deserialize)]
            struct Envelope {
                status: u16,
                body: Vec<u8>,
            }
            #[derive(Deserialize)]
            struct Created {
                id: String,
                owner: String,
            }
            let outcome: Envelope = serde_json::from_slice(bytes).map_err(unavailable)?;
            if outcome.status != 201 {
                return Ok(unknown(UnknownReason::NoDurableEvidence));
            }
            let created: Created = serde_json::from_slice(&outcome.body).map_err(unavailable)?;
            if !valid_sandbox_id(&created.id) || created.owner != identity.owner.as_str() {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            return Ok(Outcome::created(
                &identity.digest(),
                claim,
                created.id,
                None,
            ));
        }
        if row.get_bool("live").map_err(unavailable)? {
            Ok(Outcome::InProgress { operation_id: None })
        } else {
            Ok(unknown(UnknownReason::RecoveryRequired))
        }
    }

    pub(super) async fn recover_cached(
        &self,
        identity: &Identity<'_>,
        begin: Begin,
    ) -> Result<Begin, StatusCode> {
        if identity.operation != Operation::Sandbox || !matches!(begin, Begin::Existing { .. }) {
            return Ok(begin);
        }
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(unavailable)?;
        let Some(row) = Self::read_claim(&self.database, identity, &mut session).await? else {
            return Ok(begin);
        };
        // This namespace is permanently single-generation and the exact identity
        // was already accepted by the proof store.
        let cached_response = receipt(&row)?.map(<[u8]>::to_vec);
        Ok(Begin::Existing {
            operation_id: None,
            cached_response,
        })
    }

    async fn read_claim(
        database: &Database,
        identity: &Identity<'_>,
        session: &mut ClientSession,
    ) -> Result<Option<Document>, StatusCode> {
        let size =
            doc! {"$cond":[{"$eq":[{"$type":"$outcome"},"binData"]},{"$binarySize":"$outcome"},0]};
        let pipeline = vec![
            doc! {"$match":{"_id":{"subject":identity.owner.as_str(),"operation":"sandbox:create","key":identity.key}}},
            doc! {"$project":{
                "claim_id":1, "document_schema_version":1,"fingerprint":1,
                "outcome_type":{"$type":"$outcome"},
                "expired":{"$and":[{"$ne":[{"$ifNull":["$expires_at",Bson::Null]},Bson::Null]},{"$lte":["$expires_at","$$NOW"]}]},
                "retained":{"$and":[{"$eq":[{"$type":"$expires_at"},"date"]},{"$gt":["$expires_at","$$NOW"]}]},
                "live":{"$and":[{"$eq":[{"$type":"$lease_expires_at"},"date"]},{"$gt":["$lease_expires_at","$$NOW"]},{"$ne":["$recovery_required",true]}]},
                "oversize":{"$gt":[size.clone(),MAX_RECEIPT_BYTES]},
                "outcome":{"$cond":[{"$and":[{"$eq":[{"$type":"$outcome"},"binData"]},{"$lte":[size,MAX_RECEIPT_BYTES]},{"$eq":[{"$type":"$expires_at"},"date"]},{"$gt":["$expires_at","$$NOW"]}]},"$outcome","$$REMOVE"]},
            }},
        ];
        let mut cursor = database
            .collection::<Document>(super::super::operation::OPERATIONS)
            .aggregate(pipeline)
            .max_time(IO_BUDGET)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let row = cursor
            .stream(session)
            .try_next()
            .await
            .map_err(unavailable)?;
        let Some(row) = row else { return Ok(None) };
        schema::require_current(
            row.get_i64("document_schema_version")
                .map_err(unavailable)?,
        )
        .map_err(unavailable)?;
        if row.get_bool("expired").map_err(unavailable)? {
            return Ok(None);
        }
        let outcome_type = row.get_str("outcome_type").map_err(unavailable)?;
        if !matches!(outcome_type, "null" | "missing" | "binData")
            || (outcome_type == "binData" && !row.get_bool("retained").map_err(unavailable)?)
        {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok(Some(row))
    }

    async fn legacy_lifecycle(
        snapshot: &Database,
        identity: &Identity<'_>,
        query: &LookupRequest,
        session: &mut ClientSession,
    ) -> Result<Outcome, StatusCode> {
        let row = snapshot.collection::<Document>(super::super::agent_lifecycle_operations::COLLECTION)
            .find_one(doc! {"owner":identity.owner.as_str(),"key":identity.key})
            .projection(doc! {"status":1,"creation_wire":1,"sandbox_id":1,"document_schema_version":1,"input.request.sandbox_id":1,"input.request.workspace_id":1})
            .max_time(IO_BUDGET).session(&mut *session).await.map_err(unavailable)?;
        let Some(row) = row else {
            return Ok(unknown(UnknownReason::NoDurableEvidence));
        };
        schema::require_current(
            row.get_i64("document_schema_version")
                .map_err(unavailable)?,
        )
        .map_err(unavailable)?;
        let operation_id = row.get_str("_id").map_err(unavailable)?;
        if !contract::lower_hex(operation_id, 32) {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        if query
            .operation_id
            .as_deref()
            .is_some_and(|id| id != operation_id)
        {
            return Err(StatusCode::CONFLICT);
        }
        let Some(Bson::Document(wire)) = row.get("creation_wire") else {
            if row.contains_key("creation_wire") {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            return Ok(unknown(UnknownReason::LegacyFingerprintUnavailable));
        };
        if identity.algorithm != Algorithm::NyxidWire {
            return Ok(unknown(UnknownReason::LegacyFingerprintUnavailable));
        }
        if wire.get_str("deployment").map_err(unavailable)? != identity.deployment
            || wire.get_str("digest").map_err(unavailable)? != identity.wire_digest
        {
            return Err(StatusCode::CONFLICT);
        }
        let request = row
            .get_document("input")
            .and_then(|value| value.get_document("request"))
            .map_err(unavailable)?;
        if ["sandbox_id", "workspace_id"]
            .iter()
            .any(|key| !matches!(request.get(*key), None | Some(Bson::Null)))
        {
            return Ok(unknown(UnknownReason::NoDurableEvidence));
        }
        let origin = snapshot.collection::<Document>("sandboxes")
            .find_one(doc! {"owner":identity.owner.as_str(),"origin_type":"lifecycle","origin_operation_id":operation_id})
            .projection(doc! {"_id":1,"document_schema_version":1,"record.owner":1,"record.origin":1})
            .max_time(IO_BUDGET).session(session).await.map_err(unavailable)?;
        if let Some(origin) = origin {
            super::super::work_schema::require(&origin).map_err(unavailable)?;
            let id = origin.get_str("_id").map_err(unavailable)?;
            let sandbox = origin.get_document("record").map_err(unavailable)?;
            let lineage = sandbox.get_document("origin").map_err(unavailable)?;
            if !valid_sandbox_id(id)
                || sandbox.get_str("owner").map_err(unavailable)? != identity.owner.as_str()
                || lineage.get_str("type").map_err(unavailable)? != "lifecycle"
                || lineage.get_str("operation_id").map_err(unavailable)? != operation_id
                || row.get_str("sandbox_id").is_ok_and(|saved| saved != id)
            {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            return Ok(Outcome::created(
                &identity.digest(),
                operation_id,
                id.to_owned(),
                Some(operation_id.to_owned()),
            ));
        }
        if row.get_str("sandbox_id").is_ok() {
            return Ok(unknown(UnknownReason::RecoveryRequired));
        }
        match row.get_str("status").map_err(unavailable)? {
            "queued" | "running" | "ready" | "waiting_for_connection" => Ok(Outcome::InProgress {
                operation_id: Some(operation_id.to_owned()),
            }),
            _ => Ok(unknown(UnknownReason::RecoveryRequired)),
        }
    }
}

fn receipt(row: &Document) -> Result<Option<&[u8]>, StatusCode> {
    match row.get("outcome") {
        Some(Bson::Binary(binary))
            if binary.subtype == mongodb::bson::spec::BinarySubtype::Generic
                && binary.bytes.len() <= MAX_RECEIPT_BYTES as usize =>
        {
            Ok(Some(&binary.bytes))
        }
        None | Some(Bson::Null) => Ok(None),
        _ => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

fn exact_claim<'a>(
    row: &'a Document,
    expected: Option<&str>,
) -> Result<Option<&'a str>, StatusCode> {
    match row.get("claim_id") {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(id)) if contract::lower_hex(id, 32) => {
            if expected.is_some_and(|expected| expected != id) {
                Err(StatusCode::CONFLICT)
            } else {
                Ok(Some(id))
            }
        }
        _ => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

fn check_fingerprint(row: &Document, expected: &str) -> Result<(), StatusCode> {
    let Some(Bson::Binary(fingerprint)) = row.get("fingerprint") else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    if fingerprint.bytes.len() != 32
        || fingerprint.subtype != mongodb::bson::spec::BinarySubtype::Generic
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if fingerprint
        .bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        != expected
    {
        return Err(StatusCode::CONFLICT);
    }
    Ok(())
}
