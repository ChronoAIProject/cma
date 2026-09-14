use k8s_openapi::api::core::v1::Pod;
use kube::api::PostParams;

use crate::{
    HomeSnapshotExportJobInventoryEntry, HomeSnapshotExportJobObservation,
    HomeSnapshotExportJobPhase as Phase, HomeSnapshotExportJobReceipt,
    HomeSnapshotExportJobRequest, HomeSnapshotExportJobStore, HomeSnapshotExportReceipt,
    HomeSnapshotViewObservation, HomeSnapshotViewStore, Result,
};

use super::{failure, PersistentHomeVolumes};

#[path = "home_snapshot_job_cleanup.rs"]
mod cleanup;
#[path = "home_snapshot_job_identity.rs"]
mod identity;
#[path = "home_snapshot_job_spec.rs"]
mod spec;
#[path = "home_snapshot_job_transfer.rs"]
mod transfer;

use spec::invalid;

impl PersistentHomeVolumes {
    async fn read_export_job(&self, request: &HomeSnapshotExportJobRequest) -> Result<Option<Pod>> {
        self.export_pods
            .get_opt(&spec::name(request)?)
            .await
            .map_err(|error| failure("read snapshot exporter", error))
    }

    fn export_job_receipt(
        &self,
        request: &HomeSnapshotExportJobRequest,
        pod: &Pod,
    ) -> Result<HomeSnapshotExportJobReceipt> {
        identity::receipt(&self.namespace, &self.controller, request, pod)
    }

    async fn export_job_observation(
        &self,
        request: &HomeSnapshotExportJobRequest,
        receipt: HomeSnapshotExportJobReceipt,
        pod: &Pod,
        previous_view: &HomeSnapshotViewObservation,
    ) -> Result<HomeSnapshotExportJobObservation> {
        if previous_view.receipt != request.view {
            return Err(invalid());
        }
        let view = self.observe_view(&request.export, previous_view).await?;
        let phase = identity::phase(pod)?;
        if matches!(phase, Phase::Exporting | Phase::Ready) && view.bound_volume.is_none() {
            return Err(invalid());
        }
        Ok(HomeSnapshotExportJobObservation {
            receipt,
            view,
            phase,
        })
    }
}

#[async_trait::async_trait]
impl HomeSnapshotExportJobStore for PersistentHomeVolumes {
    async fn export_job_inventory(&self) -> Result<Vec<HomeSnapshotExportJobInventoryEntry>> {
        cleanup::inventory(self).await
    }

    async fn prepare_export_job(
        &self,
        request: &HomeSnapshotExportJobRequest,
    ) -> Result<HomeSnapshotExportJobObservation> {
        let desired = spec::desired(&self.namespace, &self.controller, request)?;
        let view = self
            .observe_view(
                &request.export,
                &HomeSnapshotViewObservation {
                    receipt: request.view.clone(),
                    bound_volume: None,
                },
            )
            .await?;
        let pod = match self.read_export_job(request).await? {
            Some(pod) => pod,
            None => match self
                .export_pods
                .create(&PostParams::default(), &desired)
                .await
            {
                Ok(pod) => pod,
                Err(error) => match self.read_export_job(request).await? {
                    Some(pod) => pod,
                    None => return Err(failure("schedule snapshot exporter", error)),
                },
            },
        };
        let receipt = self.export_job_receipt(request, &pod)?;
        self.export_job_observation(request, receipt, &pod, &view)
            .await
    }

    async fn observe_export_job(
        &self,
        request: &HomeSnapshotExportJobRequest,
        previous: &HomeSnapshotExportJobObservation,
    ) -> Result<HomeSnapshotExportJobObservation> {
        previous.receipt.validate_for(request)?;
        let pod = self.read_export_job(request).await?.ok_or_else(invalid)?;
        let receipt = self.export_job_receipt(request, &pod)?;
        if receipt != previous.receipt {
            return Err(invalid());
        }
        let current = self
            .export_job_observation(request, receipt, &pod, &previous.view)
            .await?;
        if matches!(
            (previous.phase, current.phase),
            (
                Phase::Ready | Phase::Failed,
                Phase::Pending | Phase::Exporting
            ) | (Phase::Exporting, Phase::Pending)
                | (Phase::Failed, Phase::Ready)
        ) {
            return Err(invalid());
        }
        Ok(current)
    }

    async fn upload_export(
        &self,
        request: &HomeSnapshotExportJobRequest,
        ready: &HomeSnapshotExportJobObservation,
        store: &dyn crate::hosted::CheckpointArchiveStore,
    ) -> Result<HomeSnapshotExportReceipt> {
        transfer::upload(self, request, ready, store).await
    }

    async fn remove_export_job(
        &self,
        request: &HomeSnapshotExportJobRequest,
        receipt: Option<&HomeSnapshotExportJobReceipt>,
    ) -> Result<()> {
        cleanup::remove(self, request, receipt).await
    }
}

#[cfg(test)]
#[path = "home_snapshot_jobs_tests.rs"]
mod tests;
