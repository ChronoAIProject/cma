use crate::{
    config::Config,
    document_storage::{maintenance::StorageMaintenance, StorageDocuments, UnavailableDocuments},
    store::MongoStores,
};
use layer2_core::{
    account_deletion::AccountStanding,
    document_artifacts::{DocumentBlobMaintenance, DocumentBlobStore, DocumentService},
    UserDirectory,
};
use std::sync::Arc;

pub(super) struct DocumentComposition {
    pub service: Arc<DocumentService>,
    pub maintenance: Option<Arc<dyn DocumentBlobMaintenance>>,
}

pub(super) fn compose(
    config: &Config,
    stores: &MongoStores,
    standing: Arc<dyn AccountStanding>,
) -> Result<DocumentComposition, String> {
    let maintenance = config
        .document_storage
        .clone()
        .map(|storage| {
            StorageMaintenance::new(storage, config.features.document_object_deletion)
                .map(|client| client as Arc<dyn DocumentBlobMaintenance>)
        })
        .transpose()?;
    let blobs: Arc<dyn DocumentBlobStore> = match config.document_storage.clone() {
        Some(config) => StorageDocuments::new(config)?,
        None => Arc::new(UnavailableDocuments),
    };
    let store = Arc::new(
        stores
            .document_artifacts
            .with_source_commits(config.features.document_source_commits),
    );
    let quota = Arc::new(store.quota(config.features.document_quotas));
    let service = DocumentService::with_storage_quota(
        store,
        blobs,
        config.features.document_revocation_reserve,
        Some(quota),
    )
    .with_account_standing(standing)
    .with_directory(UserDirectory::new(stores.users.clone()));
    Ok(DocumentComposition {
        service,
        maintenance,
    })
}
