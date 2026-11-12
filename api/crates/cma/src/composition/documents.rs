use crate::{
    config::Config,
    document_storage::{StorageDocuments, UnavailableDocuments},
    store::MongoStores,
};
use layer2_core::document_artifacts::{DocumentBlobStore, DocumentService};
use std::sync::Arc;

pub(super) fn compose(
    config: &Config,
    stores: &MongoStores,
) -> Result<Arc<DocumentService>, String> {
    let blobs: Arc<dyn DocumentBlobStore> = match config.document_storage.clone() {
        Some(config) => StorageDocuments::new(config)?,
        None => Arc::new(UnavailableDocuments),
    };
    Ok(DocumentService::with_revocation_reserve(
        stores.document_artifacts.clone(),
        blobs,
        config.features.document_revocation_reserve,
    ))
}
