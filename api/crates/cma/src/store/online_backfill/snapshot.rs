use mongodb::{
    bson::{doc, Document, Timestamp},
    error::ErrorKind,
    Database,
};

use super::{Result, INVALID, UNAVAILABLE};

pub(super) struct Snapshot<'a> {
    database: &'a Database,
    time: Timestamp,
}

impl<'a> Snapshot<'a> {
    pub(super) fn new(database: &'a Database, time: Timestamp) -> Self {
        Self { database, time }
    }

    pub(super) async fn page(
        &self,
        collection: &str,
        filter: Document,
        limit: i64,
    ) -> Result<Vec<Document>> {
        self.find(
            collection,
            filter,
            limit,
            doc! {"sort": {"_id": 1}, "hint": {"_id": 1}},
        )
        .await
    }

    pub(super) async fn one(&self, collection: &str, filter: Document) -> Result<Option<Document>> {
        self.find(collection, filter, 1, doc! {})
            .await
            .map(|mut rows| rows.pop())
    }

    pub(super) async fn find(
        &self,
        collection: &str,
        filter: Document,
        limit: i64,
        options: Document,
    ) -> Result<Vec<Document>> {
        let mut command = doc! {
            "find": collection,
            "filter": filter,
            "limit": limit,
            "batchSize": limit,
            "singleBatch": true,
            "maxTimeMS": 5000_i64,
            "readConcern": {"level": "snapshot", "atClusterTime": self.time},
        };
        command.extend(options);
        let reply = self.database.run_command(command).await.map_err(|error| {
            if matches!(error.kind.as_ref(), ErrorKind::Command(reason) if reason.code == 286) {
                "backfill snapshot expired. Start a new run to observe a fresh snapshot"
            } else {
                UNAVAILABLE
            }
        })?;
        let cursor = reply.get_document("cursor").map_err(|_| INVALID)?;
        let rows = cursor.get_array("firstBatch").map_err(|_| INVALID)?;
        if cursor.get_i64("id").ok() != Some(0) || rows.len() as i64 > limit {
            return Err(INVALID);
        }
        rows.iter()
            .map(|row| row.as_document().cloned().ok_or(INVALID))
            .collect()
    }
}
