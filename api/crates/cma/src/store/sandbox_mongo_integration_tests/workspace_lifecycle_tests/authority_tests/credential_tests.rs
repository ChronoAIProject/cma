use super::*;
use crate::auth::keys::KeyIssuer;
use crate::dependency_test_support::{Exchange, Peer};
use crate::sandbox::{
    CredentialError, ManagedSandboxCredentials, OwnerTokenSource, SandboxCredentialController,
};
use crate::store::sandbox_credentials::MongoSandboxCredentialStore;
use layer2_core::sandbox::store::{ClaimedWork, WorkAuthority};
use serde_json::{json, Value};
use std::sync::Arc;

mod acknowledgements;
mod legacy_admission_tests;
mod legacy_tests;
mod observation_tests;
mod purge_owner_tests;
mod responsibility_tests;
mod retirement_fixture;
mod retirement_tests;
mod snapshot_tests;
mod uncertainty;

struct Tokens;
#[async_trait::async_trait]
impl OwnerTokenSource for Tokens {
    async fn token_for(&self, _: &SubjectId) -> Result<Option<SecretValue>, String> {
        Ok(Some(SecretValue::new(format!(
            "fixture-{}",
            uuid::Uuid::new_v4()
        ))))
    }
}

struct Fixture {
    client: Client,
    name: String,
    key: AuthEncryptionKey,
    store: MongoSandboxStore,
    custody: Arc<MongoSandboxCredentialStore>,
    issuer: Arc<KeyIssuer>,
    controller: Arc<ManagedSandboxCredentials>,
    peer: Peer,
    claim: Sandbox,
}

impl Fixture {
    async fn new(suffix: &str) -> Self {
        Self::new_mode(suffix, true).await
    }

    async fn new_mode(suffix: &str, enabled: bool) -> Self {
        Self::new_with_image_origin(suffix, enabled, None).await
    }

    async fn new_with_image_origin(
        suffix: &str,
        enabled: bool,
        origin: Option<layer2_core::sandbox::SandboxRuntimeImageOrigin>,
    ) -> Self {
        use base64::Engine;
        use ring::rand::{SecureRandom, SystemRandom};
        let (client, name) = database(suffix).await;
        let mut bytes = [0; 32];
        SystemRandom::new().fill(&mut bytes).unwrap();
        let key = AuthEncryptionKey::from_base64(
            &base64::engine::general_purpose::STANDARD.encode(bytes),
        )
        .unwrap();
        let store = MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .with_renewable_work(enabled);
        let mut row = provisioning_sandbox(Kind::Sandbox.id());
        row.credential.reaches = vec![layer2_core::ServiceId::new("fixture-service")];
        row.profile.content.nyxid_services = row.credential.reaches.clone();
        let mint_step = SandboxWorkState::new(SandboxWorkStep::MintInitialCredential, at(-1));
        if let Some(origin) = origin {
            row.origin = layer2_core::sandbox::SandboxOrigin::Lifecycle {
                operation_id: "fixture-operation".into(),
            };
            row.profile.content.runtime_image = Some(format!(
                "registry.example/runtime@sha256:{}",
                "a".repeat(64)
            ));
            row.lifecycle.runtime_image_origin = Some(origin);
            row.profile.digest =
                layer2_core::profiles::content_digest(&row.profile.content).unwrap();
            let name = SandboxAdmissionName::Exact(row.name.clone());
            let mut admission = SandboxAdmission::new(row, name, 1, at(0)).unwrap();
            admission.sandbox.status = SandboxStatus::Installing;
            admission.sandbox.work = mint_step;
            let document = crate::store::sandboxes::admitted_sandbox_document(
                &admission.sandbox,
                0,
                &admission.name,
                &admission.admission_fingerprint,
            )
            .unwrap();
            Kind::Sandbox
                .rows(&store)
                .insert_one(document)
                .await
                .unwrap();
        } else {
            row.status = SandboxStatus::Installing;
            row.work = mint_step;
            store.create_sandbox(row).await.unwrap();
        }
        let claim = store
            .claim_sandbox_work(
                &SandboxId(Kind::Sandbox.id().into()),
                "original",
                at(0),
                at(15),
            )
            .await
            .unwrap()
            .unwrap();
        let custody = Arc::new(
            MongoSandboxCredentialStore::open(&client, &name, "work-contract", &key)
                .await
                .unwrap(),
        );
        let peer = Peer::start().await;
        let issuer = Arc::new(KeyIssuer::new(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            &peer.origin,
            "work-contract",
        ));
        let controller =
            ManagedSandboxCredentials::new(issuer.clone(), Arc::new(Tokens), custody.clone());
        Self {
            client,
            name,
            key,
            store,
            custody,
            issuer,
            controller,
            peer,
            claim,
        }
    }

    fn issue(
        &self,
        generation: u64,
    ) -> tokio::task::JoinHandle<
        Result<Result<CredentialMaterial, CredentialError>, SandboxStoreError>,
    > {
        self.issue_with(self.store.clone(), self.controller.clone(), generation)
    }

    fn issue_with(
        &self,
        store: MongoSandboxStore,
        controller: Arc<ManagedSandboxCredentials>,
        generation: u64,
    ) -> tokio::task::JoinHandle<
        Result<Result<CredentialMaterial, CredentialError>, SandboxStoreError>,
    > {
        let claim = self.claim.clone();
        tokio::spawn(async move {
            WorkAuthority::new(
                ClaimedWork::Sandbox(Box::new(claim.clone())),
                tokio::time::Instant::now(),
            )
            .run(
                async {
                    Ok(controller
                        .issue(&claim, generation, SystemTime::now())
                        .await)
                },
                || async { store.renew_sandbox_authority(&claim, &at(0), &at(15)).await },
            )
            .await
        })
    }

    async fn prepare(&mut self) -> Exchange {
        let services = self.peer.next().await;
        assert_eq!(services.uri.path(), "/api/v1/user-services");
        services
            .json(json!([{"id":"fixture-service-id","slug":"fixture-service","is_active":true}]));
        let scope = self.peer.next().await;
        assert_eq!(scope.uri.path(), "/api/v1/api-keys/scope-plan");
        let body: Value = serde_json::from_slice(&scope.body).unwrap();
        scope.json(crate::auth::keys::scope_plan_fixture(
            self.claim.owner.as_str(),
            &body,
            &["fixture-node"],
        ));
        let mint = self.peer.next().await;
        assert_eq!(mint.uri.path(), "/api/v1/api-keys");
        assert_eq!(mint.method, axum::http::Method::POST);
        mint
    }

    async fn takeover(&mut self) {
        // Store-contract test only: the two-process timing runner uses natural
        // expiry. Independent clients and an expired predecessor are real here.
        let independent = peer(&self.name).await;
        Kind::Sandbox
            .set(
                &independent,
                doc! {"work_lease_expires_at":at(-1),"record.work.lease.expires_at":at(-1)},
            )
            .await;
        self.claim = independent
            .claim_sandbox_work(&self.claim.id, "successor", at(0), at(15))
            .await
            .unwrap()
            .unwrap();
        self.store = independent;
        let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        self.custody = Arc::new(
            MongoSandboxCredentialStore::open(&client, &self.name, "work-contract", &self.key)
                .await
                .unwrap(),
        );
        self.controller = ManagedSandboxCredentials::new(
            self.issuer.clone(),
            Arc::new(Tokens),
            self.custody.clone(),
        );
    }

    async fn use_custody_app(&mut self, app: &str) {
        let mut options =
            ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
                .await
                .unwrap();
        options.app_name = Some(app.into());
        let client = Client::with_options(options).unwrap();
        self.custody = Arc::new(
            MongoSandboxCredentialStore::open(&client, &self.name, "work-contract", &self.key)
                .await
                .unwrap(),
        );
        self.controller = ManagedSandboxCredentials::new(
            self.issuer.clone(),
            Arc::new(Tokens),
            self.custody.clone(),
        );
    }

    async fn commit_failpoint(&self, app: &str, delay_ms: i32) {
        self.client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":"alwaysOn",
                "data":{"failCommands":["commitTransaction"],"appName":app,
                    "blockConnection":true,"blockTimeMS":delay_ms,
                    "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired",
                        "errmsg":"fixture lost custody acknowledgement"}}
            })
            .await
            .unwrap();
    }

    async fn clear_failpoint(&self) {
        self.client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":"off"
            })
            .await
            .unwrap();
    }

    async fn due_observation(&self) {
        self.client
            .database(&self.name)
            .collection::<Document>("sandbox_credential_issuance")
            .update_one(
                doc! {"_id":self.claim.id.as_str()},
                doc! {"$set":{"record.next_check_at":at(-1)}},
            )
            .await
            .unwrap();
    }

    async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}

fn accepted(request: Exchange) -> String {
    let secret = format!("fixture_{}", uuid::Uuid::new_v4().simple());
    request.json(json!({"id":"fixture-provider-key","full_key":&secret,
        "scopes":"proxy read","state_version":7,"allowed_service_ids":["fixture-service-id"],
        "allowed_node_ids":["fixture-node"],"allow_all_services":false,"allow_all_nodes":false,
        "allow_auto_connected_services":false}));
    secret
}
