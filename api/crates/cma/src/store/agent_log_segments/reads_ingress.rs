use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, SharedOwned,
};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{bson::Document, ClientSession};

use super::{
    corrupt, database, majority_collection, Head, MongoSandboxStore, RawDocumentBuf, ReadConcern,
    Seal, SEGMENTS,
};
use crate::store::ingress_reads::{
    error::{self, ReadFailure},
    layout::{BsonLayout, LayoutError},
    row::{self, ConversionPlan, RowSchema},
};

type SealRead<'a> = AdmittedOperation<'a, Result<SharedOwned<Seal>, ReadFailure>>;

impl Seal {
    /// Read one seal with the original session and read concern. The caller's
    /// read plan covers query/raw response/driver storage; driver bounds and
    /// detached cleanup still require their separately qualified port.
    pub(in crate::store) fn read_at_admitted<'a>(
        store: &'a MongoSandboxStore,
        head: &'a Head,
        ordinal: i64,
        session: &'a mut ClientSession,
        snapshot: bool,
        policy: &'a IngressOwnershipPolicy,
        read_plan: AllocationPlan,
    ) -> Result<SealRead<'a>, AdmittedError> {
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Transformation,
            AllocationPlan::checked(
                "seal read result",
                &[128, shared_owner_bytes::<ReadFailure>()],
            )?,
            move |scope| async move {
                let result = row::load_with_error::<SealSchema, _, _>(policy, read_plan, || async {
                    let seals = majority_collection::<RawDocumentBuf>(&database(store), SEGMENTS);
                    let find = seals.find_one(mongodb::bson::doc! {
                        "agent_id": &head.agent_id, "incarnation": &head.incarnation, "ordinal": ordinal,
                    });
                    let find = if snapshot {
                        find.read_concern(ReadConcern::snapshot())
                    } else {
                        find
                    };
                    find.session(session).await
                })
                .await;
                let value = match result {
                    Ok(Some(value))
                        if value.agent_id == head.agent_id
                            && value.incarnation == head.incarnation
                            && value.ordinal == ordinal =>
                    {
                        value
                    }
                    Ok(_) => {
                        return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(corrupt()))))
                    }
                    Err(error) => return error::finish(Err(error), &scope),
                };
                Ok(scope.initialize_owned(|| Ok(value)))
            },
        )
    }
}

impl Seal {
    /// Locate the requested seal and validate its neighboring chain in the same
    /// session. Each independent read uses the caller's full read allowance.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "enabled recording awaits driver resource custody")
    )]
    pub(in crate::store) fn containing_admitted<'a>(
        store: &'a MongoSandboxStore,
        head: &'a Head,
        seq: i64,
        session: &'a mut ClientSession,
        snapshot: bool,
        policy: &'a IngressOwnershipPolicy,
        read_plan: AllocationPlan,
    ) -> Result<SealRead<'a>, AdmittedError> {
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Transformation,
            AllocationPlan::checked(
                "containing seal result",
                &[128, shared_owner_bytes::<ReadFailure>()],
            )?,
            move |scope| async move {
                let result = row::load_with_error::<SealSchema, _, _>(policy, read_plan, || async {
                    let seals = majority_collection::<RawDocumentBuf>(&database(store), SEGMENTS);
                    let find = seals.find_one(mongodb::bson::doc! {
                        "agent_id": &head.agent_id, "incarnation": &head.incarnation, "start_seq": {"$lte": seq},
                    }).sort(mongodb::bson::doc! {"start_seq": -1});
                    let find = if snapshot { find.read_concern(ReadConcern::snapshot()) } else { find };
                    find.session(&mut *session).await
                }).await;
                let seal = match result {
                    Ok(Some(seal)) => seal,
                    Ok(None) => {
                        return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(corrupt()))))
                    }
                    Err(error) => return error::finish(Err(error), &scope),
                };
                if let Err(error) = super::validate_containing(&seal, head, seq) {
                    return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error))));
                }
                if seal.ordinal > 0 {
                    let previous = match Seal::read_at_admitted(
                        store,
                        head,
                        seal.ordinal - 1,
                        session,
                        snapshot,
                        policy,
                        read_plan,
                    )?
                    .await?
                    .transpose()
                    {
                        Ok(previous) => previous,
                        Err(error) => return Ok(error.map(Err)),
                    };
                    if let Err(error) = seal.follows(&previous) {
                        return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error))));
                    }
                }
                if seal.ordinal + 1 == head.active_ordinal {
                    if let Err(error) = seal.precedes(head) {
                        return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error))));
                    }
                } else {
                    let next = match Seal::read_at_admitted(
                        store,
                        head,
                        seal.ordinal + 1,
                        session,
                        snapshot,
                        policy,
                        read_plan,
                    )?
                    .await?
                    .transpose()
                    {
                        Ok(next) => next,
                        Err(error) => return Ok(error.map(Err)),
                    };
                    if let Err(error) = next.follows(&seal) {
                        return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error))));
                    }
                }
                Ok(scope.initialize_owned(|| Ok(seal)))
            },
        )
    }
}

struct SealSchema;

impl RowSchema for SealSchema {
    type Value = Seal;
    const INVALID_RAW: &'static str = "invalid raw log seal";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        // The persisted seal holds inline fields, Strings and binary buffers.
        let retained = layout.domain_bytes(0, shared_owner_bytes::<Seal>())?;
        // The unchanged decoder serializes BSON for the seal ceiling, then
        // validates the identity and digest. Include serialization growth,
        // overlapping BSON/serde/typed storage and malformed-field diagnostics.
        let temporary = layout
            .bson_bytes()?
            .checked_mul(3)
            .and_then(|bytes| retained.checked_mul(2)?.checked_add(bytes))
            .and_then(|bytes| layout.wire_bytes.checked_mul(4)?.checked_add(bytes))
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or(LayoutError::Structure)?;
        Ok(ConversionPlan {
            temporary_bytes: temporary,
            retained_bytes: retained,
        })
    }

    fn decode(row: Document) -> Result<Seal, SandboxStoreError> {
        Seal::read(row)
    }
}
