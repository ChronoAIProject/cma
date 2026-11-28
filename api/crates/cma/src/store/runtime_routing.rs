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

mod route;

use route::{Backend, RouteKey};

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
        let sandboxes = super::durability::majority_collection(db, "sandboxes");
        let mut rows = sandboxes
            .find(doc! {})
            .await
            .map_err(|_| "cannot read retained runtime routes")?;
        while let Some(row) = rows
            .try_next()
            .await
            .map_err(|_| "cannot read retained runtime routes")?
        {
            let Some(sandbox) = super::sandbox_quarantine::retained(&sandboxes, row).await? else {
                continue;
            };
            let generation = sandbox.lifecycle.runtime_generation_high_water.max(1);
            let key = RouteKey::new(deployment, sandbox.id.as_str(), generation)
                .map_err(|e| e.to_string())?;
            if store
                .rows
                .find_one(key.filter())
                .await
                .map_err(|_| "cannot read runtime route")?
                .is_some()
            {
                continue;
            }
            let selected = match sandbox.runtime.as_ref() {
                Some(runtime) => Backend::from_runtime_id(&runtime.runtime_id).is_agent_sandbox(),
                None if !allocate_new => false,
                None if initialized || matches!(sandbox.status, layer2_core::sandbox::SandboxStatus::Retired) => continue,
                None => return Err("a retained Sandbox has no durable provider choice; first deploy this version with opensandbox allocation before switching providers".into()),
            };
            store
                .select(sandbox.id.as_str(), generation, selected)
                .await
                .map_err(|e| e.to_string())?;
        }
        let quarantined = sandboxes
            .count_documents(doc! { (super::sandbox_quarantine::FIELD): { "$exists": true } })
            .await
            .map_err(|_| "cannot count quarantined Sandboxes")?;
        metrics::gauge!("cma_retained_sandbox_quarantined").set(quarantined as f64);
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
        let key = RouteKey::new(&self.deployment, sandbox, generation)?;
        let backend = Backend::from_allocation_choice(default_new);
        let query = key.filter();
        let update = doc! {"$setOnInsert": key.insert_fields(backend)?};
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
        route::read_backend(&row).map(Backend::is_agent_sandbox)
    }
}
