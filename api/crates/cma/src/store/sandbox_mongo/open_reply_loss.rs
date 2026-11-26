use super::MongoSandboxStore;
use layer3_adapter::hosted::HostedOpenReplyLoss;
use mongodb::bson::{doc, Document};
use mongodb::Collection;
use std::time::Duration;

const CLAIMED_FIELD: &str = "staging_open_reply_loss_claimed";

pub(crate) struct OpenReplyLossClaim {
    sandboxes: Collection<Document>,
    sandbox_id: String,
}

impl MongoSandboxStore {
    pub(crate) fn open_reply_loss_claim(&self, sandbox_id: String) -> OpenReplyLossClaim {
        OpenReplyLossClaim {
            sandboxes: self.sandboxes.clone(),
            sandbox_id,
        }
    }
}

#[async_trait::async_trait]
impl HostedOpenReplyLoss for OpenReplyLossClaim {
    async fn claim(&self) -> bool {
        // The existing Sandbox document keeps the one-shot marker across
        // worker takeovers and releases it with the fixture's own retention.
        let update = self.sandboxes.update_one(
            doc! {
                "_id": &self.sandbox_id,
                "status": "active",
                CLAIMED_FIELD: { "$exists": false },
            },
            doc! { "$set": { CLAIMED_FIELD: true } },
        );
        match tokio::time::timeout(Duration::from_secs(2), update).await {
            Ok(Ok(result)) => result.modified_count == 1,
            _ => {
                tracing::warn!(
                    "staging ACP open reply retained because its one-shot claim was unconfirmed"
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn open_reply_loss_claim_is_atomic_across_clients_and_scoped_to_an_active_sandbox() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let database = format!("cma_open_reply_loss_{}", uuid::Uuid::new_v4().simple());
        let first = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let second = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let rows = |client: &mongodb::Client| {
            super::super::majority_collection(&client.database(&database), super::super::SANDBOXES)
        };
        let one = OpenReplyLossClaim {
            sandboxes: rows(&first),
            sandbox_id: "target".into(),
        };
        let other = OpenReplyLossClaim {
            sandboxes: rows(&second),
            sandbox_id: "target".into(),
        };
        assert!(!one.claim().await);
        one.sandboxes
            .insert_many([
                doc! {"_id": "target", "status": "paused"},
                doc! {"_id": "untargeted", "status": "active"},
            ])
            .await
            .unwrap();
        assert!(!one.claim().await);
        one.sandboxes
            .update_one(doc! {"_id": "target"}, doc! {"$set": {"status": "active"}})
            .await
            .unwrap();

        let (one_won, other_won) = tokio::join!(one.claim(), other.claim());
        assert_ne!(one_won, other_won);
        assert!(!one.claim().await);
        assert!(!other.claim().await);
        let target = one
            .sandboxes
            .find_one(doc! {"_id": "target"})
            .await
            .unwrap()
            .unwrap();
        let untargeted = one
            .sandboxes
            .find_one(doc! {"_id": "untargeted"})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(target.get_bool(CLAIMED_FIELD), Ok(true));
        assert!(!untargeted.contains_key(CLAIMED_FIELD));
        first.database(&database).drop().await.unwrap();
    }
}
