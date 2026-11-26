//! Durable provider choice per Sandbox generation, recorded before external dispatch.
use futures_util::TryStreamExt;
use layer3_adapter::{hosted::RuntimeRoutingStore, AdapterError};
use mongodb::{
    bson::{doc, Document},
    options::ReturnDocument,
    Collection, Database,
};
use std::sync::Arc;

use crate::warm_pools::DefaultPoolHolds;

const COLLECTION: &str = "runtime_backend_routes";
pub(crate) struct MongoRuntimeRouting {
    rows: Collection<Document>,
    deployment: String,
    default_pool_holds: Arc<DefaultPoolHolds>,
}
impl MongoRuntimeRouting {
    #[cfg(test)]
    pub(crate) async fn open(
        db: &Database,
        deployment: &str,
        allocate_new: bool,
    ) -> Result<Arc<Self>, String> {
        Self::open_with_default_pool_holds(db, deployment, allocate_new, Arc::default()).await
    }

    pub(crate) async fn open_with_default_pool_holds(
        db: &Database,
        deployment: &str,
        allocate_new: bool,
        default_pool_holds: Arc<DefaultPoolHolds>,
    ) -> Result<Arc<Self>, String> {
        super::schema::initialize_collections(db, &[COLLECTION])
            .await
            .map_err(|e| e.to_string())?;
        let store = Arc::new(Self {
            rows: super::durability::majority_collection(db, COLLECTION),
            deployment: deployment.into(),
            default_pool_holds,
        });
        let migration_id = format!("provider-routing-initialized:{deployment}");
        let initialized = store
            .rows
            .find_one(doc! {"_id":&migration_id})
            .await
            .map_err(|_| "cannot read provider routing migration")?
            .is_some();
        // Before changing allocation policy, deploy this version with legacy creation
        // enabled. Existing unknown dispatches receive an immutable legacy route.
        let mut rows = db
            .collection::<Document>("sandboxes")
            .find(doc! {})
            .await
            .map_err(|_| "cannot read retained runtime routes")?;
        while let Some(row) = rows
            .try_next()
            .await
            .map_err(|_| "cannot read retained runtime routes")?
        {
            let sandbox =
                super::sandboxes::read_sandbox(&row).map_err(|_| "invalid retained Sandbox")?;
            let generation = sandbox.lifecycle.runtime_generation_high_water.max(1);
            let key = store
                .key(sandbox.id.as_str(), generation)
                .map_err(|e| e.to_string())?;
            if store
                .rows
                .find_one(doc! {"_id":&key})
                .await
                .map_err(|_| "cannot read runtime route")?
                .is_some()
            {
                continue;
            }
            let selected = match sandbox.runtime.as_ref() {
                Some(runtime) => runtime.runtime_id.starts_with("agent-sandbox:"),
                None if !allocate_new => false,
                None if initialized || matches!(sandbox.status, layer2_core::sandbox::SandboxStatus::Retired) => continue,
                None => return Err("a retained Sandbox has no durable provider choice; first deploy this version with opensandbox allocation before switching providers".into()),
            };
            store
                .select(sandbox.id.as_str(), generation, selected)
                .await
                .map_err(|e| e.to_string())?;
        }
        store
            .rows
            .update_one(
                doc! {"_id":&migration_id},
                doc! {"$setOnInsert":{"document_schema_version":1_i64,"deployment":deployment}},
            )
            .upsert(true)
            .await
            .map_err(|_| "cannot record provider routing migration")?;
        Ok(store)
    }
    fn key(&self, sandbox: &str, generation: u64) -> layer3_adapter::Result<String> {
        layer3_adapter::provider_request_digest(&(&self.deployment, sandbox, generation))
    }
}
#[async_trait::async_trait]
impl RuntimeRoutingStore for MongoRuntimeRouting {
    fn require_allocation_image(&self, image: &str) -> layer3_adapter::Result<()> {
        if self.default_pool_holds.image(image) {
            return Err(AdapterError::Unavailable(
                "runtime allocation is held because the configured image conflicts with durable pool authority".into(),
            ));
        }
        Ok(())
    }

    async fn select(
        &self,
        sandbox: &str,
        generation: u64,
        default_new: bool,
    ) -> layer3_adapter::Result<bool> {
        let id = self.key(sandbox, generation)?;
        let selected = if default_new {
            "agent-sandbox"
        } else {
            "opensandbox"
        };
        let generation = i64::try_from(generation)
            .map_err(|_| AdapterError::Rejected("runtime generation is out of range".into()))?;
        let query = doc! {"_id":&id};
        let update = doc! {"$setOnInsert":{"document_schema_version":1_i64,"sandbox_id":sandbox,"generation":generation,"deployment":&self.deployment,"backend":selected}};
        let row = match self
            .rows
            .find_one_and_update(query.clone(), update)
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await
        {
            Ok(Some(row)) => row,
            _ => self
                .rows
                .find_one(query)
                .await
                .map_err(|_| {
                    AdapterError::Unavailable("runtime routing store is unavailable".into())
                })?
                .ok_or_else(|| {
                    AdapterError::Unavailable("runtime provider choice is not durable".into())
                })?,
        };
        match row.get_str("backend") {
            Ok("agent-sandbox") => Ok(true),
            Ok("opensandbox") => Ok(false),
            _ => Err(AdapterError::Rejected(
                "invalid recorded runtime provider".into(),
            )),
        }
    }
}
