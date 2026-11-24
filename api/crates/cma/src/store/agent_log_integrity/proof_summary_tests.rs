use super::*;
use layer2_core::sandbox::ingress_ownership::{ClassBounds, IngressBudget};

pub(in crate::store::agent_log_integrity) fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let plan = |n| AllocationPlan::checked("summary fixture", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(
            root.capacity_bytes(),
            plan(1024 * 1024),
            plan(96 * 1024 * 1024),
        )
        .unwrap(),
    )
    .unwrap();
    (root, policy)
}

pub(in crate::store::agent_log_integrity) fn metadata(
    agent: &Agent,
    hash: [u8; HASH_BYTES],
) -> LogIntegrity {
    let mut checkpoint =
        mongodb::bson::serialize_to_document(&SemanticCheckpoint::empty()).unwrap();
    checkpoint.insert("through_seq", agent.conversation.last_seq as i64);
    checkpoint.insert("creation_seen", true);
    LogIntegrity {
        head_seq: agent.conversation.last_seq,
        entry_count: agent.conversation.last_seq,
        encoded_bytes: 100 * agent.conversation.last_seq as usize,
        head_hash: hash,
        bound_agent_revision: agent.revision,
        history_identity: history_identity(agent),
        semantic: super::super::metadata::parse_semantic(&checkpoint).unwrap(),
    }
}

pub(in crate::store::agent_log_integrity) fn read<'a>(
    store: &'a MongoSandboxStore,
    agent: &'a Agent,
    raw: &'a mongodb::bson::RawDocument,
    session: &'a mut mongodb::ClientSession,
    snapshot: bool,
    policy: &'a IngressOwnershipPolicy,
) -> Result<super::super::read::SummaryRead<'a>, AdmittedError> {
    super::super::read::read_only_summary_admitted(
        store,
        agent,
        raw,
        session,
        snapshot,
        policy,
        AllocationPlan::checked("summary source", &[50 * 1024 * 1024])?,
    )
}

#[test]
fn metadata_and_summary_keep_independent_data_and_original_errors() {
    let (root, policy) = policy();
    let plan = |n| AllocationPlan::checked("summary fixture", &[n]).unwrap();
    let idle = root.available_bytes();
    let mut semantic = mongodb::bson::serialize_to_document(&SemanticCheckpoint::empty()).unwrap();
    semantic.insert("valid", false);
    semantic.insert("conversation_id", "c".repeat(32768));
    semantic.insert(
        "queued_owner_messages",
        (0..128)
            .map(|n| {
                doc! {
                    "sequence":n+1, "content_sha256":"a".repeat(64), "nonempty":true,
                }
            })
            .collect::<Vec<_>>(),
    );
    let metadata = LogIntegrity {
        head_seq: 0,
        entry_count: 0,
        encoded_bytes: 0,
        head_hash: [0; HASH_BYTES],
        bound_agent_revision: 1,
        history_identity: [0; HASH_BYTES],
        semantic: super::super::metadata::parse_semantic(&semantic).unwrap(),
    };
    let row = doc! {INTEGRITY_FIELD:metadata.to_document().unwrap()};
    let raw = RawDocumentBuf::try_from(&row).unwrap();
    let parsed = ingress_row::decode::<SummarySchema>(&raw, &policy).unwrap();
    let copy = copy_summary(&parsed, &policy).unwrap();
    assert_eq!(copy.semantic.as_ref(), Some(&metadata.semantic));
    assert!(copy.belongs_to(&policy));
    let both = root.available_bytes();
    drop(parsed);
    assert!(root.available_bytes() > both);
    assert!(root.available_bytes() < idle);
    drop(copy);
    assert_eq!(root.available_bytes(), idle);
    let mut bad = row.clone();
    bad.get_document_mut(INTEGRITY_FIELD)
        .unwrap()
        .insert("head_seq", -1_i64);
    let expected = parse_summary_metadata(&bad).unwrap_err();
    let raw = RawDocumentBuf::try_from(&bad).unwrap();
    let pressure = policy
        .admit(IngressClass::Data, plan(policy.bounds().data_bytes()))
        .unwrap();
    assert!(matches!(
        ingress_row::decode::<SummarySchema>(&raw, &policy),
        Err(ingress_row::RowError::Refused(LayoutError::Capacity))
    ));
    drop(pressure);
    let ingress_row::RowError::Domain(error) =
        ingress_row::decode::<SummarySchema>(&raw, &policy).unwrap_err()
    else {
        panic!("original metadata error required")
    };
    assert_eq!(*error, expected);
    drop(error);
    assert_eq!(root.available_bytes(), idle);
}
