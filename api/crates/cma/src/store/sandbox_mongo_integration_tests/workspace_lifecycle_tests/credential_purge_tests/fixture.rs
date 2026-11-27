use super::*;
use futures_util::TryStreamExt;

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub store: MongoSandboxStore,
    pub parent: Sandbox,
}

impl Fixture {
    pub async fn new(prefix: &str) -> Self {
        let (client, name) = database(prefix).await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        crate::store::work_schema::floor::retain(&store, &mut session)
            .await
            .unwrap();
        session.commit_transaction().await.unwrap();
        let parent = retired_sandbox("sbx_credential_purge");
        store.create_sandbox(parent.clone()).await.unwrap();
        store
            .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
            .await
            .unwrap();
        store
            .create_workspace(workspace(&parent, "wks_retained", "retained"))
            .await
            .unwrap();
        Self {
            client,
            name,
            store,
            parent,
        }
    }

    pub fn rows(&self, name: &str) -> mongodb::Collection<Document> {
        self.client.database(&self.name).collection(name)
    }

    pub fn intent(&self, state: IssuanceState) -> CredentialIssuance {
        CredentialIssuance {
            operation_id: format!("mint_{}", uuid::Uuid::new_v4().simple()),
            sandbox_id: self.parent.id.clone(),
            owner: self.parent.owner.clone(),
            generation: 1,
            identity: IssuanceIdentity::Prepared {
                provider_identity: "a".repeat(64),
                request_digest: "b".repeat(64),
            },
            owner_authority: None,
            original_holder: "original-issuer".into(),
            original_fence: 1,
            created_at: NOW.into(),
            next_check_at: NOW.into(),
            state,
        }
    }

    pub async fn seed_intent(&self, intent: &CredentialIssuance) {
        self.rows("sandbox_credential_issuance")
            .insert_one(
                doc! {"_id":self.parent.id.as_str(), "document_schema_version":1_i64,
                "record":mongodb::bson::serialize_to_bson(intent).unwrap()},
            )
            .await
            .unwrap();
        self.rows("sandboxes")
            .update_one(
                doc! {"_id":self.parent.id.as_str()},
                doc! {"$set":{"credential_issuance_fence":1_i64}},
            )
            .await
            .unwrap();
    }

    pub async fn purge(&self) -> Result<(), SandboxStoreError> {
        self.store
            .purge_sandbox(&self.parent.id, self.parent.revision, NOW)
            .await
    }

    pub async fn snapshot(&self) -> Vec<(String, Vec<Document>)> {
        let mut snapshot = Vec::new();
        for name in [
            "sandboxes",
            "sandbox_events",
            "sandbox_usage",
            "sandbox_usage_events",
            "workspaces",
            "worktrees",
            "checkpoints",
            "sandbox_credentials",
            "sandbox_credential_revocations",
            "sandbox_credential_issuance",
        ] {
            let rows = self
                .rows(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            snapshot.push((name.into(), rows));
        }
        snapshot
    }

    pub async fn cleanup(&self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
