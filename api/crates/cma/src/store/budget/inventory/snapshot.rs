use super::{digest, projection, Limits, Result, Value, INVALID, LIMIT, READ};
use mongodb::{
    bson::{doc, Document},
    options::{Collation, CollectionOptions, Hint, ReadConcern},
    ClientSession, Database,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};

pub(super) struct Scan {
    pub selected: usize,
    pub settled: usize,
    pub rows: Vec<Value>,
    pub outcomes: BTreeMap<&'static str, usize>,
    pub source_sha256: String,
}

struct Budget {
    source_bytes: usize,
    output_bytes: usize,
    hash: Sha256,
}
impl Budget {
    fn source(
        &mut self,
        name: &str,
        id: &str,
        document: Option<&Document>,
        limits: Limits,
    ) -> Result<String> {
        let bytes = document
            .map(mongodb::bson::serialize_to_vec)
            .transpose()
            .map_err(|_| INVALID)?
            .unwrap_or_default();
        self.source_bytes = self.source_bytes.checked_add(bytes.len()).ok_or(LIMIT)?;
        if self.source_bytes > limits.source_bytes {
            return Err(LIMIT);
        }
        for part in [name.as_bytes(), id.as_bytes(), &bytes] {
            self.hash.update((part.len() as u64).to_be_bytes());
            self.hash.update(part);
        }
        Ok(digest(&bytes))
    }
}

pub(super) async fn read(
    database: &Database,
    session: &mut ClientSession,
    limits: Limits,
    #[cfg(test)] hook: &dyn super::PageHook,
) -> Result<Scan> {
    let collection = |name| {
        database.collection_with_options::<Document>(
            name,
            CollectionOptions::builder()
                .read_concern(ReadConcern::snapshot())
                .build(),
        )
    };
    let mut cursor = collection(super::super::COLLECTION)
        .find(doc! {"_id": {"$gte": "turn:", "$lt": "turn;"}})
        .sort(doc! {"_id": 1})
        .collation(Collation::builder().locale("simple").build())
        .hint(Hint::Name("_id_".into()))
        .batch_size(limits.page_size)
        .max_time(Duration::from_secs(5))
        .session(&mut *session)
        .await
        .map_err(|_| READ)?;
    let mut scan = Scan {
        selected: 0,
        settled: 0,
        rows: Vec::new(),
        outcomes: BTreeMap::new(),
        source_sha256: String::new(),
    };
    let mut budget = Budget {
        source_bytes: 0,
        output_bytes: 0,
        hash: Sha256::new(),
    };
    let mut last: Option<String> = None;
    for page in 0..limits.pages {
        let mut eof = false;
        for _ in 0..limits.page_size {
            let Some(document) = cursor.next(&mut *session).await else {
                eof = true;
                break;
            };
            let document = document.map_err(|_| READ)?;
            scan.selected += 1;
            if scan.selected > limits.rows {
                return Err(LIMIT);
            }
            let id = document.get_str("_id").map_err(|_| INVALID)?;
            if !id.starts_with("turn:") || last.as_deref().is_some_and(|prior| id <= prior) {
                return Err(INVALID);
            }
            let source = budget.source(super::super::COLLECTION, id, Some(&document), limits)?;
            let saved = projection::turn(&document)?;
            last = Some(id.to_owned());
            if saved.reservation.settled {
                scan.settled += 1;
                continue;
            }
            let agent = collection("agents")
                .find_one(doc! {"_id": saved.agent_id.as_str()})
                .collation(Collation::builder().locale("simple").build())
                .hint(Hint::Name("_id_".into()))
                .max_time(Duration::from_secs(5))
                .session(&mut *session)
                .await
                .map_err(|_| READ)?;
            let agent_source =
                budget.source("agents", saved.agent_id.as_str(), agent.as_ref(), limits)?;
            let (outcome, observed) = projection::linkage(&saved, agent.as_ref());
            let row = json!({"key": id, "saved": projection::exact_saved(&saved)?,
                "linkage": outcome, "agent": observed, "record_sha256": source,
                "agent_record_sha256": agent_source});
            budget.output_bytes = budget
                .output_bytes
                .checked_add(serde_json::to_vec(&row).map_err(|_| INVALID)?.len())
                .ok_or(LIMIT)?;
            if budget.output_bytes > limits.output_bytes {
                return Err(LIMIT);
            }
            *scan.outcomes.entry(outcome).or_default() += 1;
            scan.rows.push(row);
        }
        #[cfg(test)]
        hook.after_page(page + 1).await?;
        #[cfg(not(test))]
        let _ = page;
        if eof {
            scan.source_sha256 = format!("{:x}", budget.hash.finalize());
            return Ok(scan);
        }
    }
    Err(LIMIT)
}
