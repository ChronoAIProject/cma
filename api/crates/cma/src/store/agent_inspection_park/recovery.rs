//! The database operator may requeue one observed park after a receiver repair.
use super::*;
use mongodb::{Collection, Database};

const UNAVAILABLE: &str = "Agent inspection park recovery unavailable";

pub(crate) async fn command(
    arguments: &[String],
) -> Option<Result<serde_json::Value, &'static str>> {
    let operation = arguments.first()?.as_str();
    let (agent, retry) = match (operation, arguments) {
        ("--show-agent-inspection-park", [_, agent]) => (agent, None),
        ("--retry-agent-inspection", [_, agent, park]) => (agent, Some(park.as_str())),
        ("--show-agent-inspection-park", _) => {
            return Some(Err("usage: cma --show-agent-inspection-park AGENT"))
        }
        ("--retry-agent-inspection", _) => {
            return Some(Err("usage: cma --retry-agent-inspection AGENT PARK_ID"))
        }
        _ => return None,
    };
    Some(run(agent, retry).await)
}

async fn run(agent: &str, retry: Option<&str>) -> Result<serde_json::Value, &'static str> {
    if agent.is_empty()
        || agent.len() > 256
        || agent.chars().any(char::is_control)
        || retry.is_some_and(|park| uuid::Uuid::parse_str(park).is_err())
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
        let agents = agents(&database);
        match retry {
            Some(park_id) => {
                let requeued = consume(&agents, agent, park_id)
                    .await
                    .map_err(|_| UNAVAILABLE)?;
                Ok(serde_json::json!({"requeued": requeued}))
            }
            None => {
                let row = agents
                    .find_one(current_claim_park(agent))
                    .projection(
                        doc! {"_id": 0, format!("{FIELD}.id"): 1, format!("{FIELD}.phase"): 1,
                        format!("{FIELD}.reason"): 1, format!("{FIELD}.step"): 1,
                        format!("{FIELD}.retry"): 1, format!("{FIELD}.retry_claimed"): 1,
                        format!("{FIELD}.probe_at"): 1, format!("{FIELD}.epoch"): 1},
                    )
                    .await
                    .map_err(|_| UNAVAILABLE)?;
                serde_json::to_value(row.and_then(|row| row.get_document(FIELD).ok().cloned()))
                    .map_err(|_| UNAVAILABLE)
            }
        }
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
    let mut filter = current_claim_park(agent);
    filter.insert(format!("{FIELD}.id"), park_id);
    filter.insert(format!("{FIELD}.retry"), doc! {"$ne": true});
    let result = agents
        .update_one(filter, doc! {"$set": {format!("{FIELD}.retry"): true}})
        .await?;
    Ok(result.modified_count == 1)
}

fn current_claim_park(agent: &str) -> Document {
    let marker = format!("${FIELD}");
    doc! {
        "_id": agent,
        format!("{FIELD}.version"): 1_i64,
        format!("{FIELD}.inspection_confirmed"): {"$ne": true},
        "$expr": {"$and": [
            {"$eq": [format!("{marker}.step"), "$record.work.step"]},
            {"$eq": [format!("{marker}.fence"), "$record.work.next_fence"]},
            {"$eq": [format!("{marker}.ready_at"), "$record.work.ready_at"]},
            {"$eq": [format!("{marker}.attempts"), "$record.work.attempts"]},
        ]},
    }
}
