use crate::{
    config::Config,
    document_storage::{maintenance::StorageMaintenance, StorageDocuments, UnavailableDocuments},
    store::MongoStores,
};
use layer2_core::document_artifacts::{
    DocumentBlobMaintenance, DocumentBlobStore, DocumentService,
};
use std::sync::Arc;

pub(super) struct DocumentComposition {
    pub service: Arc<DocumentService>,
    pub maintenance: Option<Arc<dyn DocumentBlobMaintenance>>,
}

pub(super) fn compose(
    config: &Config,
    stores: &MongoStores,
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
    let deletion_supported = maintenance
        .as_ref()
        .is_some_and(|blobs| blobs.deletion_supported());
    let metadata = Arc::new(
        stores
            .document_artifacts
            .as_ref()
            .clone()
            .deletion(config.features.document_deletion, deletion_supported),
    );
    let service = DocumentService::with_deletion(
        metadata,
        blobs,
        config.features.document_revocation_reserve,
        Some(Arc::new(
            stores
                .document_artifacts
                .quota(config.features.document_quotas),
        )),
        config.features.document_deletion,
    );
    Ok(DocumentComposition {
        service,
        maintenance,
    })
}

pub(super) fn worker(
    config: &Config,
    reclaim: Arc<crate::document_artifacts::MongoDocumentReclaimStore>,
    blobs: Option<Arc<dyn DocumentBlobMaintenance>>,
) -> crate::document_artifacts::worker::DocumentMaintenanceWorker {
    let supported = blobs
        .as_ref()
        .is_some_and(|blobs| blobs.deletion_supported());
    let purge = Arc::new(reclaim.purge());
    let maintenance = blobs
        .filter(|_| config.features.document_reclaim || config.features.document_deletion)
        .map(|blobs| {
            layer2_core::document_artifacts::DocumentMaintenance::new(
                reclaim,
                blobs,
                config.features.document_reclaim,
            )
            .with_purge(
                purge.clone(),
                config.features.document_deletion && supported,
            )
        });
    let worker = crate::document_artifacts::worker::DocumentMaintenanceWorker::new(maintenance);
    if config.features.document_deletion {
        worker.with_purge(purge, supported)
    } else {
        worker
    }
}
