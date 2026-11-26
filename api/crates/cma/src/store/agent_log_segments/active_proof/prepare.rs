//! Explicit one-Agent preparation under the same snapshot/majority write fence.
use super::*;
use crate::store::{
    agent_log::read_entry,
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
    transaction_terminal::GuardedTerminal as _,
};
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogKind};
use mongodb::{options::Hint, Client, Database};

const fn unavailable() -> &'static str {
    "active log proof could not establish the original current head"
}

pub(in crate::store) async fn prepare(
    client: &Client,
    database: &Database,
    agent_id: &str,
) -> Result<i64, &'static str> {
    let mut session = client.start_session().await.map_err(|_| unavailable())?;
    start_transaction(&mut session)
        .await
        .map_err(|_| unavailable())?;
    let proof = match prepare_transaction(database, agent_id, &mut session).await {
        Ok(proof) => proof,
        Err(_) => {
            let _ = session.abort_guarded().await;
            return Err(unavailable());
        }
    };
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed | CommitOutcome::Unknown(_) => {}
        _ => return Err(unavailable()),
    }
    // A concurrent predecessor writer may have advanced the head. Report
    // success only if the exact proof is still current in one fresh snapshot.
    let mut snapshot = client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|_| unavailable())?;
    let (agent, head) = current(database, agent_id, &mut snapshot, true)
        .await
        .map_err(|_| unavailable())?;
    if head != proof.head
        || i64::try_from(agent.revision).ok() != Some(proof.agent_revision)
        || read_database(database, &head, agent.revision, &mut snapshot, true)
            .await
            .map_err(|_| unavailable())?
            != Some(proof.leaves)
    {
        return Err(unavailable());
    }
    Ok(head.revision)
}

async fn prepare_transaction(
    database: &Database,
    agent_id: &str,
    session: &mut ClientSession,
) -> Result<ActiveProof, TransactionLogError> {
    let (agent, head) = current(database, agent_id, session, false).await?;
    let existing = read_database(database, &head, agent.revision, session, false).await?;
    let leaves = scan(database, &agent, &head, session).await?;
    let proof = ActiveProof::new(&head, agent.revision, leaves)?;
    if let Some(existing) = existing {
        if existing != proof.leaves {
            return Err(corrupt().into());
        }
        return Ok(proof);
    }
    publish_prepared(database, &agent, &proof, session).await?;
    Ok(proof)
}

async fn publish_prepared(
    database: &Database,
    agent: &Agent,
    proof: &ActiveProof,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    let head = &proof.head;
    let agent_id = agent.id.as_str();
    // Exact no-op replacement participates in the same head write as every
    // publication. A competing writer must conflict or make readback stale.
    let heads = majority_collection::<Document>(database, super::super::HEADS);
    let canonical_head = bson::serialize_to_document(head).map_err(|_| corrupt())?;
    if heads
        .replace_one(canonical_head.clone(), canonical_head)
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .matched_count
        != 1
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    let agents = majority_collection::<Document>(database, "agents");
    let revision = i64::try_from(agent.revision).map_err(|_| corrupt())?;
    if agents
        .update_one(
            doc! {"_id":agent_id,"revision":revision,
            "agent_log_incarnation":&head.incarnation,"agent_log_head_revision":head.revision},
            doc! {"$set":{"agent_log_head_revision":head.revision}},
        )
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .matched_count
        != 1
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    proof.persist_database(database, session).await
}

async fn current(
    database: &Database,
    agent_id: &str,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<(Agent, Head), TransactionLogError> {
    let agents = majority_collection::<Document>(database, "agents");
    let row = crate::store::stale_turns::find_with_proof_budget(
        &agents,
        doc! {"_id":agent_id},
        session,
        snapshot,
    )
    .await
    .map_err(TransactionLogError::Driver)?
    .ok_or(SandboxStoreError::NotFound)?;
    let agent = crate::store::agents::read_agent(&row)?;
    let head = super::super::reads::metadata::read_head_from(
        database, &row, session, snapshot, None, None,
    )
    .await?;
    let projections = majority_collection::<Document>(database, super::super::PROJECTIONS);
    let projection = crate::store::stale_turns::find_with_proof_budget(
        &projections,
        doc! {"_id":agent_id},
        session,
        snapshot,
    )
    .await
    .map_err(TransactionLogError::Driver)?
    .ok_or_else(corrupt)?;
    super::super::projection::validate_projection(projection, &agent, &head)?;
    if head.active_ordinal > 0 {
        super::super::reads::metadata::seal_at_from(
            database,
            &head,
            head.active_ordinal - 1,
            session,
            snapshot,
            None,
        )
        .await?
        .precedes(&head)?;
    }
    Ok((agent, head))
}

async fn scan(
    database: &Database,
    agent: &Agent,
    head: &Head,
    session: &mut ClientSession,
) -> Result<Binary, TransactionLogError> {
    let source = majority_collection::<RawDocumentBuf>(database, "agent_log");
    let mut cursor = source
        .find(doc! {"agent_id":agent.id.as_str(),
        "seq":{"$gte":head.active_start_seq,"$lte":head.last_seq}})
        .hint(Hint::Name("agent_log_sequence".into()))
        .sort(doc! {"seq":1})
        .limit(super::super::MAX_ENTRIES + 1)
        .batch_size(64)
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut next = head.active_start_seq;
    let mut bytes = 0_i64;
    let mut leaves = Vec::new();
    while let Some(raw) = cursor
        .next(&mut *session)
        .await
        .transpose()
        .map_err(TransactionLogError::Driver)?
    {
        let length = i64::try_from(raw.as_bytes().len()).map_err(|_| corrupt())?;
        bytes = bytes
            .checked_add(length)
            .filter(|size| *size <= super::super::MAX_BYTES && length <= 16 * 1024 * 1024)
            .ok_or_else(corrupt)?;
        let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
        if entry.agent_id != agent.id
            || entry.seq != next as u64
            || next > head.last_seq
            || (next == 1) != matches!(&entry.kind, AgentLogKind::Created { .. })
        {
            return Err(corrupt().into());
        }
        if let AgentLogKind::Created {
            workspace_id,
            worktree_id,
        } = &entry.kind
        {
            if workspace_id != &agent.workspace_id || worktree_id != agent.creation_worktree_id() {
                return Err(corrupt().into());
            }
        }
        leaves.extend_from_slice(
            &digest::leaf(agent.id.as_str(), &head.incarnation, next, raw.as_bytes()).bytes,
        );
        next += 1;
    }
    if next != head.last_seq + 1 || bytes != head.active_bytes {
        return Err(corrupt().into());
    }
    Ok(digest::binary(leaves))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::agent_log_segments::{mongo_tests::fixtures, schema};

    #[tokio::test]
    #[ignore = "requires a disposable replica set"]
    async fn active_proof_first_creation_and_erasure_share_an_identity() {
        for erase_first in [true, false] {
            let (client, name) = fixtures::database("active_proof_erase").await;
            let database = client.database(&name);
            schema::migrate(&database).await.unwrap();
            let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
            let mut agent = fixtures::agent();
            agent.status = layer2_core::sandbox::AgentStatus::Ready;
            agent.conversation.conversation_id = Some("erase-fence-fixture".into());
            agent.conversation.runtime_generation = Some(1);
            let entries = (1..=276)
                .map(|seq| layer2_core::sandbox::AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: "2026-11-01T00:00:00Z".into(),
                    kind: match seq {
                        1 => AgentLogKind::Created {
                            workspace_id: agent.workspace_id.clone(),
                            worktree_id: agent.worktree_id.clone(),
                        },
                        2 => AgentLogKind::ConversationOpened {
                            conversation_id: "erase-fence-fixture".into(),
                            runtime_generation: 1,
                        },
                        _ => AgentLogKind::ModeChanged {
                            mode: "fixture".into(),
                        },
                    },
                })
                .collect::<Vec<_>>();
            let (agent, entries) = fixtures::install(&store, agent, entries, 1, 256).await;
            fixtures::certify(&store, &agent, &entries).await;
            fixtures::certify_output(&store, &agent, &entries).await;
            use layer2_core::sandbox::store::AgentLogStore as _;
            let valid = AgentLogKind::ModeChanged {
                mode: "before-erasure".into(),
            };
            let valid_identity = layer2_core::sandbox::store::AgentLogPublicationIdentity {
                key: "before-erasure".into(),
                appended_at: "2026-11-05T00:00:00Z".into(),
                fingerprint: super::super::super::publication::request_digest(&valid)
                    .unwrap()
                    .bytes
                    .try_into()
                    .unwrap(),
            };
            store
                .append_agent_log_keyed(&agent.id, &valid_identity, valid)
                .await
                .unwrap();
            let row = store
                .agents
                .find_one(doc! {"_id":agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            let agent = crate::store::agents::read_agent(&row).unwrap();
            use crate::store::migrations::agent_log_active_proofs as migration;
            for (_, index) in migration::inspect(&database).await.unwrap() {
                migration::create(&database, index).await.unwrap();
            }
            store.active_log_proofs = true;
            let proofs = database.collection::<Document>(COLLECTION);
            assert!(proofs
                .find_one(doc! {"_id":agent.id.as_str()})
                .await
                .unwrap()
                .is_none());
            let sources = database.collection::<Document>("agent_log");
            let row = sources
                .find_one(doc! {"agent_id":agent.id.as_str(),"seq":260_i64})
                .await
                .unwrap()
                .unwrap();
            let row_id = row.get("_id").unwrap().clone();
            let erase = || {
                crate::store::account_erasure::erase_source_log::erase_source_log(
                    &database,
                    std::slice::from_ref(&row),
                    std::slice::from_ref(&row_id),
                    doc! {"_id": &row_id},
                )
            };
            // The initial preparation starts before the erase with no sidecar.
            let mut session = client.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            let (source_agent, head) = current(&database, agent.id.as_str(), &mut session, false)
                .await
                .unwrap();
            let leaves = scan(&database, &source_agent, &head, &mut session)
                .await
                .unwrap();
            let proof = ActiveProof::new(&head, source_agent.revision, leaves).unwrap();
            if erase_first {
                assert_eq!(erase().await.unwrap(), 1);
                assert!(
                    publish_prepared(&database, &source_agent, &proof, &mut session)
                        .await
                        .is_err()
                );
                let _ = session.abort_guarded().await;
            } else {
                publish_prepared(&database, &source_agent, &proof, &mut session)
                    .await
                    .unwrap();
                assert!(matches!(
                    commit_transaction(&mut session).await,
                    CommitOutcome::Committed
                ));
                assert_eq!(erase().await.unwrap(), 1);
            }
            let fence = proofs
                .find_one(doc! {"_id":agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(fence.get_bool("invalidated_for_erasure").ok(), Some(true));
            assert!(prepare(&client, &database, agent.id.as_str())
                .await
                .is_err());
            let kind = AgentLogKind::ModeChanged {
                mode: "after-erasure".into(),
            };
            let identity = layer2_core::sandbox::store::AgentLogPublicationIdentity {
                key: "after-erasure".into(),
                appended_at: "2026-11-05T00:00:00Z".into(),
                fingerprint: super::super::super::publication::request_digest(&kind)
                    .unwrap()
                    .bytes
                    .try_into()
                    .unwrap(),
            };
            assert!(store
                .append_agent_log_keyed(&agent.id, &identity, kind)
                .await
                .is_err());
            database.drop().await.unwrap();
        }
    }
    #[tokio::test]
    #[ignore = "requires a built CMA executable and an authenticated disposable replica set"]
    async fn active_proof_operator_cli_uses_generated_isolated_role() {
        let (client, name) = fixtures::database("active_proof_cli").await;
        let database = client.database(&name);
        schema::migrate(&database).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, entries) = fixtures::segmented_with_width(&store, 1, 256, 20).await;
        fixtures::certify(&store, &agent, &entries).await;
        use crate::store::migrations::agent_log_active_proofs as migration;
        for (_, index) in migration::inspect(&database).await.unwrap() {
            migration::create(&database, index).await.unwrap();
        }
        let executable =
            std::env::var("CMA_TEST_CMA_EXECUTABLE").expect("exact built CMA executable");
        let printed = std::process::Command::new(&executable)
            .args(["--print-agent-log-operator-role", "active-proof"])
            .env("CHRONO_MONGODB_DATABASE", &name)
            .output()
            .unwrap();
        assert!(printed.status.success(), "printed operator role");
        let role: Document = serde_json::from_slice(&printed.stdout).unwrap();
        database.run_command(role).await.unwrap();
        let user = "active-proof-operator";
        let password = uuid::Uuid::new_v4().simple().to_string();
        database
            .run_command(doc! {"createUser":user,"pwd":&password,
            "roles":[{"role":"cma_am_log_operator","db":&name}]})
            .await
            .unwrap();
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let mut operator_uri = url::Url::parse(&uri).unwrap();
        operator_uri.set_username(user).unwrap();
        operator_uri.set_password(Some(&password)).unwrap();
        let options = operator_uri
            .query_pairs()
            .filter(|(key, _)| key != "authSource")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        operator_uri.set_query(None);
        operator_uri
            .query_pairs_mut()
            .extend_pairs(options)
            .append_pair("authSource", &name);
        let operator = Client::with_uri_str(operator_uri.as_str()).await.unwrap();
        assert!(operator
            .database(&name)
            .collection::<Document>(super::super::super::PUBLICATIONS)
            .find_one(doc! {})
            .await
            .is_err());
        let prepared = std::process::Command::new(executable)
            .args(["--agent-log-active-proof-prepare", agent.id.as_str()])
            .env("CHRONO_MONGODB_URI", operator_uri.as_str())
            .env("CHRONO_MONGODB_DATABASE", &name)
            .env("CHRONO_MONGODB_AUTHORITY_MODE", "isolated")
            .output()
            .unwrap();
        assert!(
            prepared.status.success(),
            "isolated preparation command refused"
        );
        let receipt: serde_json::Value = serde_json::from_slice(&prepared.stdout).unwrap();
        assert_eq!(receipt["operation"], "active_proof_prepare");
        let row = database
            .collection::<Document>(COLLECTION)
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let proof: ActiveProof = bson::deserialize_from_document(row).unwrap();
        proof.validate().unwrap();
        assert_eq!(
            proof.head.revision,
            receipt["head_revision"].as_i64().unwrap()
        );
        database.drop().await.unwrap();
    }
}
