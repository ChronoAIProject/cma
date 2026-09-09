use kube::api::{DeleteParams, ListParams, Preconditions};

use crate::{
    AdapterError, HomeSnapshotExportJobInventoryEntry, HomeSnapshotExportJobReceipt,
    HomeSnapshotExportJobRequest, Result,
};

use super::{failure, identity, invalid, spec, PersistentHomeVolumes};

pub(super) async fn inventory(
    store: &PersistentHomeVolumes,
) -> Result<Vec<HomeSnapshotExportJobInventoryEntry>> {
    let mut params = ListParams::default()
        .labels(&format!("{}={}", spec::LABEL, store.controller))
        .limit(200);
    let mut cursors = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    loop {
        let page = store
            .export_pods
            .list(&params)
            .await
            .map_err(|error| failure("list snapshot exporters", error))?;
        for pod in page.items {
            let request = identity::recorded_request(&pod)?;
            let receipt = store.export_job_receipt(&request, &pod)?;
            if !names.insert(receipt.pod_reference.clone()) {
                return Err(invalid());
            }
            result.push(HomeSnapshotExportJobInventoryEntry { request, receipt });
        }
        let Some(cursor) = page.metadata.continue_.filter(|value| !value.is_empty()) else {
            return Ok(result);
        };
        if !cursors.insert(cursor.clone()) {
            return Err(invalid());
        }
        params = params.continue_token(&cursor);
    }
}

pub(super) async fn remove(
    store: &PersistentHomeVolumes,
    request: &HomeSnapshotExportJobRequest,
    receipt: Option<&HomeSnapshotExportJobReceipt>,
) -> Result<()> {
    request.validate()?;
    if let Some(receipt) = receipt {
        receipt.validate_for(request)?;
    }
    let Some(pod) = store.read_export_job(request).await? else {
        return Ok(());
    };
    let found = store.export_job_receipt(request, &pod)?;
    if receipt.is_some_and(|receipt| *receipt != found) {
        return Err(invalid());
    }
    let version = pod
        .metadata
        .resource_version
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid)?;
    if pod.metadata.deletion_timestamp.is_none() {
        let params = DeleteParams {
            preconditions: Some(Preconditions {
                uid: Some(found.pod_generation.clone()),
                resource_version: Some(version),
            }),
            ..DeleteParams::default()
        };
        match store
            .export_pods
            .delete(&found.pod_reference, &params)
            .await
        {
            Ok(_) => (),
            Err(kube::Error::Api(response)) if response.is_not_found() => return Ok(()),
            Err(error) => return Err(failure("release exact snapshot exporter", error)),
        }
    }
    match store.read_export_job(request).await? {
        None => Ok(()),
        Some(current)
            if current
                .metadata
                .uid
                .as_deref()
                .is_some_and(|uid| !uid.is_empty() && uid != found.pod_generation) =>
        {
            Ok(())
        }
        Some(_) => Err(AdapterError::Unavailable(
            "snapshot exporter release is still pending".into(),
        )),
    }
}
