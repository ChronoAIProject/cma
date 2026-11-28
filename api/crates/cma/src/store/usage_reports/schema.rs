use super::*;
use layer2_core::sandbox_metrics::Sort;
use mongodb::{options::IndexOptions, IndexModel};

impl ReportingStore {
    pub(super) async fn initialize(&self) -> Result<()> {
        let collections = [
            JOBS,
            ROWS,
            CHARTS,
            CONTROL,
            super::super::sandbox_metric_bindings::COLLECTION,
            super::super::sandbox_metric_rollups::COLLECTION,
            super::super::sandbox_usage_rollups::COLLECTION,
        ];
        super::super::schema::initialize_collections(&self.db, &collections)
            .await
            .map_err(unavailable)?;
        self.index(CONTROL, doc! {"expires_at":1}, false, true)
            .await?;
        for name in collections {
            if name != CONTROL {
                self.index(name, doc! {"expires_at":1}, false, true).await?;
            }
        }
        self.index(JOBS, doc! {"query_hash":1}, true, false).await?;
        self.index(
            JOBS,
            doc! {"reserved":1,"cleanup_at":1,"_id":1},
            false,
            false,
        )
        .await?;
        self.index(JOBS, doc! {"reserved":1,"expires_at":1}, false, false)
            .await?;
        self.index(
            JOBS,
            doc! {"principal":1,"state":1,"lease_until":1},
            false,
            false,
        )
        .await?;
        self.index(
            JOBS,
            doc! {"state":1,"lease_until":1,"created_at":1},
            false,
            false,
        )
        .await?;
        self.index(ROWS, doc! {"report_id":1,"sandbox_id":1}, true, false)
            .await?;
        self.index(
            ROWS,
            doc! {"report_id":1,"owner":1,"sandbox_id":1},
            false,
            false,
        )
        .await?;
        for sort in Sort::ALL {
            for direction in [1, -1] {
                let mut keys = doc! {"report_id":1};
                keys.insert(format!("nulls.{}", sort.key()), 1);
                keys.insert(format!("sorts.{}", sort.key()), direction);
                keys.insert("sandbox_id", direction);
                self.index(ROWS, keys, false, false).await?;
            }
        }
        self.index(
            CHARTS,
            doc! {"report_id":1,"metric":1,"sandbox_id":1,"chunk":1},
            true,
            false,
        )
        .await?;
        self.index(
            CHARTS,
            doc! {"report_id":1,"metric":1,"owner":1,"sandbox_id":1,"chunk":1},
            false,
            false,
        )
        .await?;
        self.index(
            CHARTS,
            doc! {"report_id":1,"metric":1,"chunk":1,"rank":-1},
            false,
            false,
        )
        .await?;
        self.index(
            CHARTS,
            doc! {"report_id":1,"group":1,"metric":1,"series_id":1,"chunk":1},
            true,
            false,
        )
        .await?;
        self.index(
            CHARTS,
            doc! {"report_id":1,"group":1,"metric":1,"rank":-1,"series_id":1},
            false,
            false,
        )
        .await?;
        super::super::sandbox_metric_bindings::initialize(self).await?;
        super::super::sandbox_metric_rollups::initialize(self).await?;
        super::super::sandbox_usage_rollups::initialize(self).await?;
        for key in [
            "admission",
            "observer",
            "rollups",
            "replay",
            "diagnostics",
            super::retirement::SLOT,
            "report:0",
            "report:1",
            "vm:0",
            "vm:1",
        ] {
            self.collection(CONTROL).update_one(doc!{"_id":key},doc!{"$setOnInsert":{"document_schema_version":1_i64,"revision":0_i64,"lease_until":0_i64,"holder":""}})
                .upsert(true).await.map_err(unavailable)?;
        }
        Ok(())
    }
    pub(crate) async fn index(
        &self,
        collection: &str,
        keys: Document,
        unique: bool,
        ttl: bool,
    ) -> Result<()> {
        let options = IndexOptions::builder()
            .unique(unique)
            .expire_after(if ttl { Some(Duration::ZERO) } else { None })
            .build();
        self.collection(collection)
            .create_index(IndexModel::builder().keys(keys).options(options).build())
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}
