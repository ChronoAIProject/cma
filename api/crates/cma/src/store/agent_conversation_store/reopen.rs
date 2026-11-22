use layer2_core::sandbox::store::{AgentReopenGuard, SandboxStoreError};
use layer2_core::sandbox::Agent;
use mongodb::{bson::doc, ClientSession};

use super::{ApplyError, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};

pub(in crate::store) async fn validate_and_fence(
    store: &MongoSandboxStore,
    agent: &Agent,
    guard: &AgentReopenGuard,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let row = store
        .sandboxes
        .find_one(doc! { "_id": agent.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let parent = super::super::sandboxes::read_sandbox(&row).map_err(ApplyError::Domain)?;
    validate_parent_and_fence(store, agent, guard, &parent, session).await
}

// The parent must come from this transaction. Its allocation owner does not
// replace either guard validation or the write that conflicts with takeover.
pub(super) async fn validate_parent_and_fence(
    store: &MongoSandboxStore,
    agent: &Agent,
    guard: &AgentReopenGuard,
    parent: &layer2_core::sandbox::Sandbox,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    guard
        .validate(parent, agent, &layer2_core::now_rfc3339())
        .map_err(|_| {
            ApplyError::Domain(SandboxStoreError::InvalidTransaction(
                "reopen output authority is stale",
            ))
        })?;
    // A real write prevents lifecycle replacement/lease takeover from committing
    // concurrently with a receipt based on an obsolete transaction snapshot.
    let locked = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(doc! { "_id": parent.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": super::signed(parent.revision, "reopen parent revision")
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))? }),
            doc! { "$inc": { "agent_conversation_reopen_fence": 1_i64 } },
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if locked.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

use crate::store::ingress_reads::{
    error::{self, ReadFailure},
    layout::{BsonLayout, LayoutError},
    row::{self, ConversionPlan, RowSchema},
};
use layer2_core::sandbox::{
    ingress_ownership::{
        sizing::shared_owner_bytes, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
        IngressOwnershipPolicy, SharedOwned,
    },
    Sandbox,
};
use mongodb::bson::{Document, RawDocumentBuf};

type ParentRead<'a> = AdmittedOperation<'a, Result<SharedOwned<Sandbox>, ReadFailure>>;

/// Read ownership does not replace the guard and transactional write fence.
/// The read allowance still needs driver and detached-cleanup qualification.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "enabled recording awaits driver resource custody")
)]
pub(in crate::store) fn read_parent_admitted<'a>(
    store: &'a MongoSandboxStore,
    agent: &'a Agent,
    session: &'a mut ClientSession,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<ParentRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked(
            "reopen parent result",
            &[shared_owner_bytes::<ReadFailure>()],
        )?,
        move |scope| async move {
            let result = row::load_with_error::<ParentSchema, _, _>(policy, read_plan, || async {
                store
                    .sandboxes
                    .clone_with_type::<RawDocumentBuf>()
                    .find_one(doc! {"_id": agent.sandbox_id.as_str()})
                    .session(session)
                    .await
            })
            .await;
            match result {
                Ok(Some(parent)) => Ok(scope.initialize_owned(|| Ok(parent))),
                Ok(None) => Ok(scope
                    .initialize_owned(|| Err(ReadFailure::Domain(SandboxStoreError::Conflict)))),
                Err(error) => error::finish(Err(error), &scope),
            }
        },
    )
}

struct ParentSchema;

impl RowSchema for ParentSchema {
    type Value = Sandbox;
    const INVALID_RAW: &'static str = "invalid raw reopen parent";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        // Sandbox bounds its allocated vector members and nested domain nodes.
        // Count the actual row, including containers that validation may reject.
        let retained =
            layout.domain_bytes(size_of::<Sandbox>(), shared_owner_bytes::<Sandbox>())?;
        // Ordinary-work decoding clones the record. Admission replay may clone
        // the Sandbox; retained finalization may serialize JSON/content digests.
        // Include simultaneous BSON/serde/domain copies, growth and diagnostics.
        let temporary = layout
            .bson_bytes()?
            .checked_mul(4)
            .and_then(|bytes| retained.checked_mul(3)?.checked_add(bytes))
            .and_then(|bytes| layout.wire_bytes.checked_mul(16)?.checked_add(bytes))
            .and_then(|bytes| bytes.checked_add(8192))
            .ok_or(LayoutError::Structure)?;
        Ok(ConversionPlan {
            temporary_bytes: temporary,
            retained_bytes: retained,
        })
    }

    fn decode(row: Document) -> Result<Sandbox, SandboxStoreError> {
        crate::store::sandboxes::read_sandbox(&row)
    }
}
