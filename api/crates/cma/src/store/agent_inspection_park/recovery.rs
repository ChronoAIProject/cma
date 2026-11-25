//! The database operator may requeue one observed park after a receiver repair.
use super::*;
use mongodb::{Collection, Database};

const UNAVAILABLE: &str = "Agent inspection park recovery unavailable";

pub(crate) async fn command(agent: &str, park_id: &str) -> Result<bool, &'static str> {
    if agent.is_empty()
        || agent.len() > 256
        || agent.chars().any(char::is_control)
        || uuid::Uuid::parse_str(park_id).is_err()
    {
        return Err("invalid Agent or inspection park identity");
    }
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| UNAVAILABLE)?;
    let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| UNAVAILABLE)?;
    let mode = std::env::var("CHRONO_MONGODB_AUTHORITY_MODE").ok();
    let authority = cma_service_auth::mongodb_authority::MongoAuthorityMode::parse(mode.as_deref())
        .map_err(|_| UNAVAILABLE)?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let client = mongodb::Client::with_uri_str(uri)
            .await
            .map_err(|_| UNAVAILABLE)?;
        let database = client.database(&name);
        crate::store::isolation::verify_with_authority(&database, authority)
            .await
            .map_err(|_| UNAVAILABLE)?;
        consume(&agents(&database), agent, park_id)
            .await
            .map_err(|_| UNAVAILABLE)
    })
    .await
    .map_err(|_| UNAVAILABLE)?
}

fn agents(database: &Database) -> Collection<Document> {
    crate::store::durability::majority_collection(database, "agents")
}

pub(in crate::store) async fn consume(
    agents: &Collection<Document>,
    agent: &str,
    park_id: &str,
) -> mongodb::error::Result<bool> {
    let marker = format!("${FIELD}");
    let result = agents
        .update_one(
            doc! {
                "_id": agent, format!("{FIELD}.version"): 1_i64, format!("{FIELD}.id"): park_id,
                format!("{FIELD}.retry"): {"$ne": true},
                "$expr": {"$and": [
                    {"$eq": [format!("{marker}.step"), "$record.work.step"]},
                    {"$eq": [format!("{marker}.fence"), "$record.work.next_fence"]},
                ]},
            },
            doc! {"$set": {format!("{FIELD}.retry"): true}},
        )
        .await?;
    Ok(result.modified_count == 1)
}
