//! Majority-durable Bot admission, immutable pins, and worker revision fences.
use futures_util::TryStreamExt;
use layer2_core::{
    bots::{Bot, BotError, BotState, BotStore},
    SubjectId,
};
use mongodb::{bson::doc, options::IndexOptions, Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};
mod admission;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BotDocument {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i32,
    owner: String,
    creation_key_hash: String,
    revision: i64,
    runnable: bool,
    deleted: bool,
    bot: Bot,
}
impl BotDocument {
    fn new(bot: &Bot) -> Result<Self, BotError> {
        bot.validate()?;
        Ok(Self {
            id: bot.bot_id.clone(),
            document_schema_version: 1,
            owner: bot.owner.to_string(),
            creation_key_hash: bot.creation_key_hash.clone(),
            revision: i64::try_from(bot.revision).map_err(|_| BotError::Conflict)?,
            runnable: bot.work.is_some()
                || matches!(bot.state, BotState::Active | BotState::Disabled),
            deleted: bot.state == BotState::Deleted,
            bot: bot.clone(),
        })
    }
    fn decode(self) -> Result<Bot, BotError> {
        self.bot.validate().map_err(|_| BotError::Unavailable)?;
        if self.document_schema_version != 1
            || self.id != self.bot.bot_id
            || self.owner != self.bot.owner.as_str()
            || self.creation_key_hash != self.bot.creation_key_hash
            || self.revision
                != i64::try_from(self.bot.revision).map_err(|_| BotError::Unavailable)?
            || self.deleted != (self.bot.state == BotState::Deleted)
            || self.runnable
                != (self.bot.work.is_some()
                    || matches!(self.bot.state, BotState::Active | BotState::Disabled))
        {
            return Err(BotError::Unavailable);
        }
        Ok(self.bot)
    }
}
pub struct MongoBotStore {
    client: Client,
    records: Collection<BotDocument>,
}
pub(in crate::store) fn read_retained_bot(row: &mongodb::bson::Document) -> Result<Bot, BotError> {
    mongodb::bson::deserialize_from_document::<BotDocument>(row.clone())
        .map_err(|_| BotError::Unavailable)?
        .decode()
}
pub(in crate::store) fn retained_bot_document(
    bot: &Bot,
) -> Result<mongodb::bson::Document, BotError> {
    mongodb::bson::serialize_to_document(&BotDocument::new(bot)?).map_err(|_| BotError::Unavailable)
}
impl MongoBotStore {
    pub async fn open(client: &Client, database: &str) -> Result<Self, BotError> {
        let records = super::durability::majority_collection(&client.database(database), "bots");
        records
            .create_indexes([
                IndexModel::builder()
                    .keys(doc! {"owner":1,"creation_key_hash":1})
                    .options(
                        IndexOptions::builder()
                            .unique(true)
                            .name("bot_owner_creation".to_owned())
                            .build(),
                    )
                    .build(),
                IndexModel::builder()
                    .keys(doc! {"owner":1,"deleted":1,"_id":1})
                    .build(),
                IndexModel::builder()
                    .keys(doc! {"runnable":1,"_id":1})
                    .build(),
            ])
            .await
            .map_err(|_| BotError::Unavailable)?;
        Ok(Self {
            client: client.clone(),
            records,
        })
    }
    async fn rows(
        &self,
        filter: mongodb::bson::Document,
        limit: usize,
    ) -> Result<Vec<Bot>, BotError> {
        let mut cursor = self
            .records
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(limit.min(200) as i64)
            .await
            .map_err(|_| BotError::Unavailable)?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.try_next().await.map_err(|_| BotError::Unavailable)? {
            rows.push(row.decode()?);
        }
        Ok(rows)
    }
}
#[async_trait::async_trait]
impl BotStore for MongoBotStore {
    async fn insert_catalogue(&self, bot: &Bot) -> Result<(), BotError> {
        admission::insert(self, bot).await
    }

    async fn insert(&self, bot: &Bot) -> Result<(), BotError> {
        self.records
            .insert_one(BotDocument::new(bot)?)
            .await
            .map(|_| ())
            .map_err(|error| {
                if super::sandbox_mongo::is_duplicate_key(&error) {
                    BotError::Conflict
                } else {
                    BotError::Unavailable
                }
            })
    }
    async fn get(&self, id: &str) -> Result<Option<Bot>, BotError> {
        self.records
            .find_one(doc! {"_id":id})
            .await
            .map_err(|_| BotError::Unavailable)?
            .map(BotDocument::decode)
            .transpose()
    }
    async fn creation(&self, owner: &SubjectId, key_hash: &str) -> Result<Option<Bot>, BotError> {
        self.records
            .find_one(doc! {"owner":owner.as_str(),"creation_key_hash":key_hash})
            .await
            .map_err(|_| BotError::Unavailable)?
            .map(BotDocument::decode)
            .transpose()
    }
    async fn list(
        &self,
        owner: &SubjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bot>, BotError> {
        let mut filter = doc! {"owner":owner.as_str(),"deleted":false};
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt":after});
        }
        self.rows(filter, limit).await
    }
    async fn replace(&self, expected: u64, bot: &Bot) -> Result<(), BotError> {
        let current = self.get(&bot.bot_id).await?.ok_or(BotError::NotFound)?;
        if current.revision != expected {
            return Err(BotError::Conflict);
        }
        bot.validate_replacement(&current)?;
        let revision = i64::try_from(expected).map_err(|_| BotError::Conflict)?;
        let result = self
            .records
            .replace_one(
                doc! {"_id":&bot.bot_id,"revision":revision,"document_schema_version":1},
                BotDocument::new(bot)?,
            )
            .await
            .map_err(|_| BotError::Unavailable)?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(BotError::Conflict)
        }
    }
    async fn pending(&self, after: Option<&str>, limit: usize) -> Result<Vec<Bot>, BotError> {
        let mut filter = doc! {"runnable":true};
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt":after});
        }
        self.rows(filter, limit).await
    }
}
