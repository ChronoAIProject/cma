use futures_util::TryStreamExt;
use layer2_core::bridge::ingress::{CallbackError, StoredInbox};
use mongodb::bson::{doc, DateTime};

use super::MongoCallbackInbox;

impl MongoCallbackInbox {
    /// Advance by the durable row ID even if that row fails strict decoding.
    /// Only the command transaction can remove work by binding its correlation.
    pub(crate) async fn pending_channel_inputs(
        &self,
        after: Option<&str>,
    ) -> Result<Vec<String>, CallbackError> {
        let now = DateTime::now();
        let mut filter = doc! {
            "_id":{"$type":"string"}, "admission":"accepted", "command_correlation":{"$exists":false},
            "expires_at":{"$gt":now},
            "$or":[{"worker_next_at":{"$exists":false}},{"worker_next_at":{"$lte":now}}],
        };
        if let Some(after) = after {
            filter.insert("_id", doc! {"$type":"string","$gt":after});
        }
        let mut cursor = self
            .inbox
            .find(filter)
            .projection(doc! {"_id":1_i32})
            .sort(doc! {"_id":1_i32})
            .limit(32)
            .await
            .map_err(|_| CallbackError::Unavailable)?;
        let mut page = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|_| CallbackError::Unavailable)?
        {
            let id = document
                .get_str("_id")
                .map_err(|_| CallbackError::Unavailable)?
                .to_owned();
            page.push(id);
        }
        Ok(page)
    }

    pub(crate) async fn channel_input(
        &self,
        id: &str,
    ) -> Result<Option<StoredInbox>, CallbackError> {
        self.inbox.find_one(doc! {"_id":id,"admission":"accepted","command_correlation":{"$exists":false},"expires_at":{"$gt":DateTime::now()}})
            .await.map_err(|_| CallbackError::Unavailable)?.as_ref().map(super::decode).transpose()
    }

    /// Scheduling metadata never changes authenticated acceptance or supplies
    /// effect authority. Unknown admission commits are retried by durable key.
    pub(crate) async fn defer_channel_input(
        &self,
        id: &str,
        delay: std::time::Duration,
        state: &'static str,
    ) -> Result<(), CallbackError> {
        let delay_ms = i64::try_from(delay.as_millis())
            .map_err(|_| CallbackError::Bounds)?
            .clamp(100, 30_000);
        self.inbox.update_one(doc! {"_id":id,"command_correlation":{"$exists":false}},
            doc! {"$set":{"worker_next_at":DateTime::from_millis(DateTime::now().timestamp_millis().saturating_add(delay_ms)),"worker_state":state}},
        ).await.map_err(|_| CallbackError::Unavailable)?;
        Ok(())
    }
}
