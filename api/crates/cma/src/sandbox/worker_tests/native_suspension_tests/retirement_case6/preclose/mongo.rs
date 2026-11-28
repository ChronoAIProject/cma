use super::*;
use crate::store::{
    agent_ownership::MongoAgentOwnership, native_retirement_action::capture::BoundCapture,
    MongoSandboxStore,
};
use layer2_core::sandbox::store::{AgentStore, AgentWorkerStore, SandboxStore, SandboxWorkerStore};
use layer3_adapter::hosted::HostedControlTransport;
use layer3_adapter::wire::RequestFrame;
use mongodb::{
    bson::{doc, Bson, Document},
    options::{ClientOptions, ReadConcern},
    Client, Collection,
};
use uuid::Uuid;

const CAPTURE: &str = "native_retirement_capture_v1";
const ACTION: &str = "native_retirement_action_v1";

struct Fixture {
    admin: Client,
    store: MongoSandboxStore,
    parent: Sandbox,
    child: Agent,
    authority: Arc<Authority>,
    app: String,
    name: String,
}

fn member() -> Member {
    Member {
        instance_id: "child-owner".into(),
        protocol: PROTOCOL_VERSION,
        relay_address: "http://127.0.0.1:1".into(),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: 2,
    }
}

impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let name = format!("retirement_capture_{}", Uuid::new_v4().simple());
        let app = name.clone();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.retry_writes = Some(false);
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (_, mut parent, children, _) = active_siblings().await;
        let now = SystemTime::now();
        let at = rfc3339_at(now);
        parent.revision = 1;
        store.create_sandbox(parent.clone()).await.unwrap();
        parent.begin_retirement(at.clone()).unwrap();
        store.save_sandbox(1, parent.clone()).await.unwrap();
        let parent = store
            .claim_sandbox_work(
                &parent.id,
                "parent-worker",
                at.clone(),
                rfc3339_at(now + Duration::from_secs(180)),
            )
            .await
            .unwrap()
            .unwrap();
        let mut child = children.into_iter().next().unwrap();
        child.revision = 1;
        child.conversation.last_seq = 0;
        child.requested_stop_reason = Some(layer2_core::sandbox::AgentStopReason::SandboxRetired);
        child.work.schedule(AgentWorkStep::CloseProcess, at.clone());
        store.create_agent(child.clone()).await.unwrap();
        let owners = MongoAgentOwnership::open(&admin, &name).await.unwrap();
        let coordinator = OwnershipCoordinator::register(Arc::new(owners), member())
            .await
            .unwrap();
        let authority = coordinator.acquire(&child.id).await.unwrap().unwrap();
        let store = store
            .enable_agent_ownership("child-owner".into())
            .await
            .unwrap();
        let child = authority
            .run(store.claim_agent_work(
                &child.id,
                "child-worker",
                at,
                rfc3339_at(now + Duration::from_secs(120)),
            ))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        Self {
            admin,
            store,
            parent,
            child,
            authority,
            app,
            name,
        }
    }

    fn collection(&self, name: &str) -> Collection<Document> {
        self.admin.database(&self.name).collection(name)
    }

    async fn parent_row(&self) -> Document {
        self.collection("sandboxes")
            .find_one(doc! {"_id":self.parent.id.as_str()})
            .read_concern(ReadConcern::majority())
            .await
            .unwrap()
            .unwrap()
    }

    async fn fault(&self, times: i32, command: &str, mut data: Document) -> i64 {
        data.insert("appName", self.app.clone());
        data.insert("failCommands", [command]);
        let reply = self
            .admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":times}, "data":data,
            })
            .await
            .unwrap();
        reply
            .get_i64("count")
            .or_else(|_| reply.get_i32("count").map(i64::from))
            .unwrap()
    }

    async fn clear_fault(&self) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":"off",
            })
            .await
            .unwrap();
    }

    async fn cleanup(self) {
        self.clear_fault().await;
        self.admin.database(&self.name).drop().await.unwrap();
    }
}

fn phase(row: &Document) -> &str {
    row.get_document(CAPTURE)
        .unwrap()
        .get_array("slots")
        .unwrap()[0]
        .as_document()
        .unwrap()
        .get_document("state")
        .unwrap()
        .get_str("phase")
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_capture_bind_checks_claim_owner_and_ambiguous_commit() {
    let f = Fixture::new().await;
    let mut unclaimed = f.parent.clone();
    unclaimed.work.lease = None;
    assert_eq!(
        f.store
            .offer_retirement_capture(&unclaimed, &f.child.id)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let offer = f
        .store
        .offer_retirement_capture(&f.parent, &f.child.id)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .retirement_capture_offer(&f.parent, &f.child)
            .await
            .unwrap(),
        offer
    );
    let attempt = Uuid::new_v4();
    assert!(matches!(
        f.store
            .bind_retirement_capture(&f.child, offer, attempt)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    let mut stale = f.child.clone();
    stale.work.lease.as_mut().unwrap().fence += 1;
    assert!(matches!(
        f.authority
            .run(f.store.bind_retirement_capture(&stale, offer, attempt))
            .await
            .unwrap(),
        Err(SandboxStoreError::Conflict)
    ));
    let owners = f.collection("cma_agent_manager_owners");
    let owner_filter = doc! {"_id":f.child.id.as_str()};
    let saved_owner = owners
        .find_one(owner_filter.clone())
        .await
        .unwrap()
        .unwrap();
    owners
        .update_one(
            owner_filter.clone(),
            doc! {"$set":{"expires_at":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert!(matches!(
        f.authority
            .run(f.store.bind_retirement_capture(&f.child, offer, attempt))
            .await
            .unwrap(),
        Err(SandboxStoreError::Conflict)
    ));
    owners.replace_one(owner_filter, saved_owner).await.unwrap();
    assert_eq!(phase(&f.parent_row().await), "offered");

    f.fault(
        5,
        "commitTransaction",
        doc! {"errorCode":50, "errorLabels":["UnknownTransactionCommitResult"]},
    )
    .await;
    let outcome = f
        .authority
        .run(f.store.bind_retirement_capture(&f.child, offer, attempt))
        .await
        .unwrap();
    f.clear_fault().await;
    assert!(matches!(outcome, Err(SandboxStoreError::Unavailable(_))));
    assert_eq!(phase(&f.parent_row().await), "offered");

    f.fault(
        5,
        "commitTransaction",
        doc! {"writeConcernError":{"code":50,"errmsg":"fixture lost acknowledgement"}},
    )
    .await;
    let bound = f
        .authority
        .run(f.store.bind_retirement_capture(&f.child, offer, attempt))
        .await
        .unwrap();
    f.clear_fault().await;
    assert!(bound.is_ok(), "{:?}", bound.err());
    assert_eq!(phase(&f.parent_row().await), "bound");
    assert!(f
        .authority
        .run(f.store.bind_retirement_capture(&f.child, offer, attempt))
        .await
        .unwrap()
        .is_ok());
    assert!(matches!(
        f.authority
            .run(
                f.store
                    .bind_retirement_capture(&f.child, offer, Uuid::new_v4())
            )
            .await
            .unwrap(),
        Err(SandboxStoreError::Conflict)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_capture_completes_after_revocation_and_cancelled_waiter() {
    let f = Fixture::new().await;
    let offer = f
        .store
        .offer_retirement_capture(&f.parent, &f.child.id)
        .await
        .unwrap();
    let bound = Arc::new(
        f.authority
            .run(
                f.store
                    .bind_retirement_capture(&f.child, offer, Uuid::new_v4()),
            )
            .await
            .unwrap()
            .unwrap(),
    );
    let capture = Arc::new(receive(&f, &bound, f.authority.clone(), "404 Not Found").await);
    let evidence = capture.sealed().unwrap();
    let child = f.store.get_agent(&f.child.id).await.unwrap().unwrap();
    let mut parent_before = f.parent_row().await;
    parent_before.remove(CAPTURE);

    f.fault(1, "update", doc! {"errorCode":2}).await;
    let refused = f.store.complete_retirement_capture(&bound, evidence).await;
    f.clear_fault().await;
    assert!(matches!(refused, Err(SandboxStoreError::Unavailable(_))));
    assert_eq!(phase(&f.parent_row().await), "bound");

    let prior = f
        .fault(
            1,
            "update",
            doc! {"blockConnection":true, "blockTimeMS":500},
        )
        .await;
    let waiter = {
        let store = f.store.clone();
        let capture = capture.clone();
        let bound = bound.clone();
        tokio::spawn(async move {
            store
                .complete_retirement_capture(&bound, capture.sealed().unwrap())
                .await
        })
    };
    f.admin
        .database("admin")
        .run_command(doc! {
            "waitForFailPoint":"failCommand", "timesEntered":prior+1, "maxTimeMS":5000,
        })
        .await
        .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    f.clear_fault().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while phase(&f.parent_row().await) != "observed" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let receipt = f.parent_row().await;
    assert_eq!(
        f.store.complete_retirement_capture(&bound, evidence).await,
        Ok(())
    );
    assert_eq!(f.parent_row().await, receipt);
    // A separate synthetic transport lease presents a different actual reply
    // with the same tuple. It cannot replace the first durable observation.
    let simulated =
        OwnershipCoordinator::register(Arc::new(MemoryOwnershipStore::default()), member())
            .await
            .unwrap();
    let transport_authority = simulated.acquire(&f.child.id).await.unwrap().unwrap();
    assert_eq!(transport_authority.token, f.authority.token);
    let conflicting = receive(&f, &bound, transport_authority, "503 Service Unavailable").await;
    assert_eq!(
        f.store
            .complete_retirement_capture(&bound, conflicting.sealed().unwrap())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(f.parent_row().await, receipt);
    let mut parent_after = receipt.clone();
    parent_after.remove(CAPTURE);
    assert_eq!(parent_before, parent_after);
    assert_eq!(f.store.get_agent(&child.id).await.unwrap(), Some(child));

    f.collection("sandboxes")
        .update_one(
            doc! {"_id": f.parent.id.as_str()},
            doc! {"$set": {ACTION: Bson::Null}},
        )
        .await
        .unwrap();
    let held = f.parent_row().await;
    assert_eq!(
        f.store.complete_retirement_capture(&bound, evidence).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(f.parent_row().await, held);
    f.cleanup().await;
}

async fn receive(
    f: &Fixture,
    bound: &BoundCapture,
    authority: Arc<Authority>,
    response: &'static str,
) -> receiver_failure::ReceiverFailureCapture {
    receive_for(&f.parent, bound, authority, response).await
}

async fn receive_for(
    parent: &Sandbox,
    bound: &BoundCapture,
    authority: Arc<Authority>,
    response: &'static str,
) -> receiver_failure::ReceiverFailureCapture {
    let (transport, exchanges, server) = receiver_with_status(response).await;
    let runtime = RuntimeHandle::new(&parent.runtime.as_ref().unwrap().runtime_id);
    let request = RequestFrame::RuntimeStatus(layer3_adapter::RuntimeStatusRequest {
        sandbox_id: parent.id.to_string(),
        generation: 1,
        home_identity: "fixture".into(),
    });
    let (capture, attempt) = bound.capture(authority.run(transport.exchange(&runtime, request)));
    let _ = attempt.await;
    assert!(authority.receiver_cancelled());
    assert_eq!(&*exchanges.lock().unwrap(), &["challenge"]);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    capture
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_capture_proves_lost_observation_acknowledgement() {
    let f = Fixture::new().await;
    let offer = f
        .store
        .offer_retirement_capture(&f.parent, &f.child.id)
        .await
        .unwrap();
    let bound = f
        .authority
        .run(
            f.store
                .bind_retirement_capture(&f.child, offer, Uuid::new_v4()),
        )
        .await
        .unwrap()
        .unwrap();
    let capture = receive(&f, &bound, f.authority.clone(), "404 Not Found").await;
    f.fault(
        1,
        "update",
        doc! {"writeConcernError":{"code":64,"errmsg":"fixture lost acknowledgement"}},
    )
    .await;
    let result = f
        .store
        .complete_retirement_capture(&bound, capture.sealed().unwrap())
        .await;
    f.clear_fault().await;
    assert_eq!(result, Ok(()));
    assert_eq!(phase(&f.parent_row().await), "observed");
    f.cleanup().await;
}

mod consumption;

mod delivery;

pub(in crate::sandbox::worker_tests) mod composed;

#[cfg(unix)]
mod process_crash;

mod controls;
