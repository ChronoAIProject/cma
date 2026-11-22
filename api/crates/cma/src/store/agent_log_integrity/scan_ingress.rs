//! Fresh Agent rows retained for same-snapshot proof and scanner composition.
use super::*;
use crate::store::ingress_reads::{
    error::{self, ReadFailure},
    layout::{BsonLayout, LayoutError},
    row::{self, ConversionPlan, ReadError, RowSchema},
};
use layer2_core::sandbox::ingress_ownership::{
    AdmittedError, AdmittedOperation, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    SharedOwned,
};

/// Both values come from one find. This is retained input, not an integrity
/// proof, format-aware route selection, or transaction authority.
#[derive(Debug)]
pub(in crate::store) struct IntegrityAgent {
    pub(in crate::store) agent: SharedOwned<Agent>,
    pub(in crate::store) raw: SharedOwned<RawDocumentBuf>,
}

type AgentRead<'a> = AdmittedOperation<
    'a,
    Result<Option<crate::store::agent_log_integrity::read::IntegrityAgent>, ReadFailure>,
>;

impl MongoSandboxStore {
    /// Preserve the fresh Agent predicate/session/concern used by proof readers
    /// and scanners. The supplied read plan still needs driver qualification.
    pub(in crate::store) fn integrity_agent_read_admitted<'a>(
        &'a self,
        id: &'a AgentId,
        session: &'a mut mongodb::ClientSession,
        snapshot: bool,
        policy: &'a IngressOwnershipPolicy,
        read_plan: AllocationPlan,
    ) -> Result<AgentRead<'a>, AdmittedError> {
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Transformation,
            AllocationPlan::checked("integrity Agent operation", &[])?,
            move |scope| async move {
                let raw = match row::load_raw_with_error(policy, read_plan, || async {
                    let rows = self.agents.clone_with_type::<RawDocumentBuf>();
                    let find = rows.find_one(doc! {"_id":id.as_str()});
                    let find = if snapshot {
                        find.read_concern(ReadConcern::snapshot())
                    } else {
                        find
                    };
                    find.session(session).await
                })
                .await
                {
                    Ok(Some(raw)) => raw,
                    Ok(None) => return Ok(scope.initialize_owned(|| Ok(None))),
                    Err(error) => return error::finish(Err(error), &scope),
                };
                let agent = match row::decode::<AgentSchema>(&raw, policy) {
                    Ok(agent) => agent,
                    Err(error) => return error::finish(Err(ReadError::Row(error)), &scope),
                };
                Ok(scope.initialize_owned(|| Ok(Some(IntegrityAgent { agent, raw }))))
            },
        )
    }
}

struct AgentSchema;

impl RowSchema for AgentSchema {
    type Value = Agent;
    const INVALID_RAW: &'static str = "invalid raw integrity Agent";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        crate::store::agent_conversation_store::agent_conversion_plan(layout)
    }

    fn decode(document: Document) -> Result<Agent, SandboxStoreError> {
        crate::store::agents::read_agent(&document)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::proof::summary_tests::policy;
    use super::*;
    use crate::store::agents::{agent_document, read_agent};
    use crate::store::sandbox_maintenance::mongo_integration_tests::{
        active_sandbox, database, fixture, ready_agent,
    };
    use crate::store::sandbox_mongo::start_transaction;

    fn plan() -> AllocationPlan {
        AllocationPlan::checked("fresh Agent source", &[50 * 1024 * 1024]).unwrap()
    }

    #[test]
    fn actual_agent_rows_keep_independent_data_and_original_schema_validation() {
        let agent = ready_agent(&active_sandbox("sbx_agent_shape"), "agt_agent_shape");
        let (root, policy) = policy();
        let idle = root.available_bytes();
        for version in [1_i64, 2] {
            let mut document = agent_document(&agent).unwrap();
            document.insert("document_schema_version", version);
            if version == 2 {
                document.insert("agent_log_format", "segmented_v1");
                document.insert(
                    "agent_log_incarnation",
                    crate::store::agent_log_segments::incarnation(&agent),
                );
                document.insert("agent_log_head_revision", 1_i64);
            }
            let raw = RawDocumentBuf::try_from(&document).unwrap();
            let owned = row::decode::<AgentSchema>(&raw, &policy).unwrap();
            assert_eq!(*owned, agent);
            let clone = owned.clone();
            let held = root.available_bytes();
            drop(owned);
            assert_eq!(root.available_bytes(), held);
            drop(clone);
            document.insert("last_seq", -1_i64);
            let expected = read_agent(&document).unwrap_err();
            let raw = RawDocumentBuf::try_from(&document).unwrap();
            let row::RowError::Domain(error) =
                row::decode::<AgentSchema>(&raw, &policy).unwrap_err()
            else {
                panic!("owned decoder error required")
            };
            assert_eq!(*error, expected);
            drop(error);
            assert_eq!(root.available_bytes(), idle);
        }
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn fresh_agent_keeps_same_transaction_replacement_and_raw_data_owners() {
        let (client, name, store) = database().await;
        let (_, first) = fixture(&store, "sbx_fresh_agent").await;
        let mut next = first.clone();
        next.revision += 1;
        next.profile.content.system_prompt = "x".repeat(128 * 1024);
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let old = store
            .integrity_agent_read_admitted(&first.id, &mut session, false, &policy, plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        store
            .agents
            .replace_one(
                doc! {"_id":first.id.as_str()},
                agent_document(&next).unwrap(),
            )
            .session(&mut session)
            .await
            .unwrap();
        let new = store
            .integrity_agent_read_admitted(&first.id, &mut session, false, &policy, plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        let old_agent = old.as_ref().unwrap().agent.clone();
        let old_raw = old.as_ref().unwrap().raw.clone();
        assert_eq!(*old_agent, first);
        assert_eq!(*new.as_ref().unwrap().agent, next);
        assert!(new.as_ref().unwrap().agent.requested_bytes() > old_agent.requested_bytes());
        assert_eq!(
            read_agent(&Document::try_from(old_raw.as_ref()).unwrap()).unwrap(),
            first
        );
        drop((old, new));
        let data_held = policy.budget(IngressClass::Data).available_bytes();
        let held = root.available_bytes();
        drop(old_raw);
        assert!(root.available_bytes() > held);
        assert_eq!(
            policy.budget(IngressClass::Data).available_bytes(),
            data_held
        );
        session.abort_transaction().await.unwrap();
        let restored = store
            .integrity_agent_read_admitted(&first.id, &mut session, false, &policy, plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert_eq!(*restored.as_ref().unwrap().agent, first);
        drop((restored, old_agent, session));
        assert_eq!(root.available_bytes(), idle);
        client.database(&name).drop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn fresh_agent_and_summary_share_the_pinned_row_and_size_later_rows_independently() {
        use super::super::super::{
            proof::summary_tests::metadata, read::read_only_summary_admitted,
        };
        let (client, name, store) = database().await;
        let (_, mut first) = fixture(&store, "sbx_fresh_proof").await;
        first.conversation.last_seq = 0;
        first.conversation.conversation_id = None;
        first.conversation.runtime_generation = None;
        store
            .agent_log
            .delete_many(doc! {"agent_id":first.id.as_str()})
            .await
            .unwrap();
        let mut document = agent_document(&first).unwrap();
        document.insert(
            INTEGRITY_FIELD,
            metadata(&first, [0; HASH_BYTES]).to_document().unwrap(),
        );
        store
            .agents
            .replace_one(doc! {"_id":first.id.as_str()}, document)
            .await
            .unwrap();
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let mut pinned = client.start_session().snapshot(true).await.unwrap();
        let retained = store
            .integrity_agent_read_admitted(&first.id, &mut pinned, true, &policy, plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        let old = retained.as_ref().unwrap();
        let timestamp = pinned.snapshot_time();
        let other =
            mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
                .await
                .unwrap();
        let rows = other
            .database(&name)
            .collection::<Document>(store.agents.name());
        let mut next = first.clone();
        next.revision += 1;
        next.profile.content.system_prompt = "n".repeat(128 * 1024);
        let mut document = agent_document(&next).unwrap();
        document.insert(
            INTEGRITY_FIELD,
            metadata(&next, [0; HASH_BYTES]).to_document().unwrap(),
        );
        rows.replace_one(doc! {"_id":first.id.as_str()}, document)
            .await
            .unwrap();
        let proof = read_only_summary_admitted(
            &store,
            &old.agent,
            &old.raw,
            &mut pinned,
            true,
            &policy,
            plan(),
        )
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
        assert!(proof.is_some());
        assert_eq!(pinned.snapshot_time(), timestamp);
        let mut session = client.start_session().await.unwrap();
        let available = policy.budget(IngressClass::Data).available_bytes();
        let pressure = policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "only an earlier Agent fits",
                    &[available - old.agent.requested_bytes() - 4096],
                )
                .unwrap(),
            )
            .unwrap();
        let same = store
            .integrity_agent_read_admitted(&first.id, &mut pinned, true, &policy, plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert_eq!(*same.as_ref().unwrap().agent, first);
        drop(same);
        assert!(matches!(
            store
                .integrity_agent_read_admitted(&first.id, &mut session, false, &policy, plan())
                .unwrap()
                .await,
            Err(AdmittedError::Refused(_))
        ));
        drop(pressure);
        let new = store
            .integrity_agent_read_admitted(&first.id, &mut session, false, &policy, plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert_eq!(*new.as_ref().unwrap().agent, next);
        assert_eq!(*old.agent, first);
        drop((new, proof, pinned, session));
        // The original result still owns both old representations here.
        assert!(root.available_bytes() < idle);
        drop(retained);
        assert_eq!(root.available_bytes(), idle);
        drop(rows);
        other.shutdown().await;
        client.database(&name).drop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn fresh_agent_refusal_and_corruption_leave_the_callers_transaction_active() {
        let (client, name, store) = database().await;
        let (_, agent) = fixture(&store, "sbx_fresh_errors").await;
        let valid = agent_document(&agent).unwrap();
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let mut session = client.start_session().await.unwrap();
        for fault in ["missing", "query", "version", "deep", "data"] {
            let mut document = valid.clone();
            match fault {
                "query" => {
                    document.insert("last_seq", -1_i64);
                }
                "version" => {
                    document.insert("document_schema_version", 3_i64);
                }
                "deep" => {
                    let mut nested = doc! {"leaf":true};
                    for _ in 0..66 {
                        nested = doc! {"nested":nested};
                    }
                    document.insert("extra", nested);
                }
                _ => {}
            }
            store
                .agents
                .replace_one(doc! {"_id":agent.id.as_str()}, document.clone())
                .await
                .unwrap();
            let id = if fault == "missing" {
                AgentId("agt_absent_fresh".into())
            } else {
                agent.id.clone()
            };
            let pressure = (fault == "data").then(|| {
                policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked(
                            "Agent Data pressure",
                            &[policy.bounds().data_bytes()],
                        )
                        .unwrap(),
                    )
                    .unwrap()
            });
            start_transaction(&mut session).await.unwrap();
            store
                .agent_log
                .insert_one(doc! {"_id":"fresh-marker"})
                .session(&mut session)
                .await
                .unwrap();
            let result = store
                .integrity_agent_read_admitted(&id, &mut session, false, &policy, plan())
                .unwrap()
                .await;
            match fault {
                "missing" => assert!(result.unwrap().transpose().unwrap().is_none()),
                "deep" => assert!(matches!(result, Err(AdmittedError::Unsupported(_)))),
                "data" => assert!(matches!(result, Err(AdmittedError::Refused(_)))),
                _ => {
                    let error = result.unwrap().transpose().unwrap_err();
                    assert!(
                        matches!(&*error, ReadFailure::Domain(actual) if *actual == read_agent(&document).unwrap_err())
                    );
                }
            }
            drop(pressure);
            super::super::super::proof::ingress::assert_open_transaction(
                &store,
                &mut session,
                "fresh-marker",
            )
            .await;
            session.abort_transaction().await.unwrap();
            assert_eq!(
                store
                    .agents
                    .find_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap(),
                Some(document)
            );
            assert_eq!(root.available_bytes(), idle);
        }
        client.database(&name).drop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn fresh_agent_admission_and_driver_error_keep_the_original_lifetimes() {
        let (client, name, _) = database().await;
        let mut options = mongodb::options::ClientOptions::parse(
            std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        )
        .await
        .unwrap();
        options.app_name = Some("cma-fresh-agent-error".into());
        let reader = mongodb::Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&reader, &name).await.unwrap();
        let (_, agent) = fixture(&store, "sbx_fresh_driver").await;
        let mut session = reader.start_session().await.unwrap();
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let operation = store
            .integrity_agent_read_admitted(&agent.id, &mut session, false, &policy, plan())
            .unwrap();
        drop(operation);
        assert_eq!(root.available_bytes(), idle);
        let pressure = policy
            .admit(
                IngressClass::Transformation,
                AllocationPlan::checked(
                    "operation pressure",
                    &[policy.bounds().transformation_bytes()],
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            store.integrity_agent_read_admitted(&agent.id, &mut session, false, &policy, plan()),
            Err(AdmittedError::Refused(_))
        ));
        drop(pressure);
        start_transaction(&mut session).await.unwrap();
        let admin = client.database("admin");
        admin
            .run_command(
                doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                    "failCommands":["find"], "appName":"cma-fresh-agent-error", "errorCode":112,
                    "errorLabels":["TransientTransactionError"],
                }},
            )
            .await
            .unwrap();
        let result = store
            .integrity_agent_read_admitted(&agent.id, &mut session, false, &policy, plan())
            .unwrap()
            .await;
        admin
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .unwrap();
        let error = result.unwrap().transpose().unwrap_err();
        let ReadFailure::Driver(driver) = &*error else {
            panic!("original driver required")
        };
        assert!(driver.contains_label("TransientTransactionError"));
        let pointer = std::ptr::from_ref(driver.kind.as_ref());
        let moved = error.map(|e| match e {
            ReadFailure::Driver(driver) => driver,
            _ => unreachable!(),
        });
        assert_eq!(std::ptr::from_ref(moved.kind.as_ref()), pointer);
        assert!(moved.belongs_to(&policy));
        drop(moved);
        assert_eq!(root.available_bytes(), idle);
        let _ = session.abort_transaction().await;
        drop((session, store));
        reader.shutdown().await;
        client.database(&name).drop().await.unwrap();
    }
}
