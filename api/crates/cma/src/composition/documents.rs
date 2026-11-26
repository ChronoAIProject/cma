use crate::{
    config::Config,
    document_artifacts::ReclamationWorker,
    document_storage::{maintenance::StorageMaintenance, StorageDocuments, UnavailableDocuments},
    store::MongoStores,
};
use layer2_core::{
    account_deletion::AccountStanding,
    document_artifacts::{DocumentBlobStore, DocumentService},
    UserDirectory,
};
use std::sync::Arc;

pub(super) struct DocumentComposition {
    pub service: Arc<DocumentService>,
    pub reclaimer: Option<ReclamationWorker>,
}

pub(super) async fn compose(
    config: &Config,
    stores: &MongoStores,
    standing: Arc<dyn AccountStanding>,
) -> Result<DocumentComposition, String> {
    if config.features.document_deletion
        && (!config.features.document_quotas || !config.features.document_source_commits)
    {
        return Err(
            "document deletion requires the source accounting writer and quota baseline".into(),
        );
    }
    let blobs: Arc<dyn DocumentBlobStore> = match config.document_storage.clone() {
        Some(config) => StorageDocuments::new(config)?,
        None => Arc::new(UnavailableDocuments),
    };
    let namespace = config
        .document_storage
        .as_ref()
        .map(crate::document_storage::namespace_sha256)
        .transpose()?;
    let store = stores
        .document_artifacts
        .with_source_commits(config.features.document_source_commits)
        .with_generation_writes(config.features.document_reclamation)
        .with_storage_namespace(namespace.clone());
    if config.features.document_reclamation
        && !(config.features.document_source_commits && config.features.document_quotas)
    {
        return Err("Document reclamation requires source commits and quotas".into());
    }
    store
        .quota(config.features.document_quotas)
        .verify_storage(namespace.as_deref(), config.features.document_reclamation)
        .await
        .map_err(|error| format!("document storage binding: {error}"))?;
    let reclaimer =
        if config.features.document_reclamation && config.features.document_object_deletion {
            let storage = config
                .document_storage
                .clone()
                .ok_or("Document reclamation requires storage maintenance")?;
            let storage = StorageMaintenance::new(storage, true)?;
            Some(
                ReclamationWorker::prepare(store.clone(), storage)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
    let quota = Arc::new(store.quota(config.features.document_quotas));
    let service = DocumentService::with_storage_quota(
        Arc::new(store),
        blobs,
        config.features.document_revocation_reserve,
        Some(quota),
    )
    .with_deletion_enabled(config.features.document_deletion)
    .with_account_standing(standing)
    .with_directory(UserDirectory::new(stores.users.clone()));
    Ok(DocumentComposition { service, reclaimer })
}
