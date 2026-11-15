//! Lifetime ownership for participation and interrupted-admission repair.
use super::{
    config,
    drain::Control,
    membership::{Membership, Worker},
    partition::Partitions,
    store::Store,
};
use crate::{http::ManagementState, ingress::routing_store::PlanStore, Config};
use tokio::task::JoinHandle;

pub(crate) struct Runtime {
    pub partitions: Partitions,
    _membership: Worker,
    repair: JoinHandle<()>,
}
impl Runtime {
    pub async fn verify_legacy(
        state: &ManagementState,
        origin: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if state.records.database().is_some() {
            let store = Store::new((*state.records).clone(), state.master_keys.clone())?;
            super::protocol::enforce(&store,origin,false).await.map_err(|_|"replica protocol floor requires replica mode and a compatible deployment identity")?;
        }
        Ok(())
    }

    pub async fn start(
        config: &Config,
        state: &ManagementState,
        control: Control,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if !config.routing.recovery_enabled || !config.routing.admission_enabled {
            return Err("replica mode requires durable routed admission and recovery".into());
        }
        state.readiness.require_replica_membership();
        let store = Store::new((*state.records).clone(), state.master_keys.clone())?;
        let database = store
            .records
            .database()
            .ok_or("replica mode requires direct Mongo")?;
        tokio::time::timeout(
            config::STORE_TIMEOUT,
            crate::records::mongo::verify_replicas(&database),
        )
        .await??;
        let address = config::direct_address()?;
        if reqwest::Url::parse(&address)?.port() != Some(config.listen.port()) {
            return Err("replica advertised port must match gateway listener".into());
        }
        super::protocol::enforce(&store, &config.management.public_url, true).await?;
        let (membership, worker) = Membership::join(
            store.clone(),
            &config.management.public_url,
            state.readiness.clone(),
            address,
            control.clone(),
        )
        .await?;
        let partitions = Partitions::new(store.clone(), membership);
        let plans = PlanStore::new((*state.records).clone(), state.master_keys.clone())
            .with_admission_queue(std::sync::Arc::new(partitions.clone()));
        super::backfill::run(&store, &plans, &partitions.membership.deployment).await?;
        let repair = tokio::spawn(async move {
            let mut cursor = None;
            loop {
                let page = tokio::select! {
                    biased;
                    _ = control.requested()=>break,
                    page = plans.recover_all_page(cursor.as_deref(),32)=>page,
                };
                match page {
                    Ok(page) => {
                        if !page.issues.is_empty() {
                            tracing::warn!(
                                count = page.issues.len(),
                                "replica admission repair found unreadable retained records"
                            );
                        }
                        if page.issues.is_empty() {
                            cursor = page.next_cursor;
                        }
                    }
                    Err(_) => tracing::warn!(
                        "replica admission repair unavailable; retaining scan cursor"
                    ),
                }
                tokio::select! {
                    _ = control.requested()=>break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250))=>{},
                }
            }
        });
        Ok(Self {
            partitions,
            _membership: worker,
            repair,
        })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.repair.abort();
    }
}
