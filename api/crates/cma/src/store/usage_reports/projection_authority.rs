use super::aggregate_projections::{GENERATION_FIELD, INVALIDATED_FIELD, MANIFEST_FIELD};
use super::*;
use cma_am_reporting::ProjectionManifest;
use layer2_core::sandbox_metrics::Report;

impl ReportingStore {
    #[cfg(test)]
    pub(super) async fn projected_generation(&self, report: &Report) -> Result<Option<i64>> {
        Ok(self
            .projected_family(report, "chart_total")
            .await?
            .map(|manifest| manifest.generation))
    }

    pub(super) async fn projected_family(
        &self,
        report: &Report,
        family: &str,
    ) -> Result<Option<ProjectionManifest>> {
        Ok(self
            .projected_manifest(report)
            .await?
            .filter(|manifest| manifest.has_family(family)))
    }

    pub(super) async fn projected_manifest(
        &self,
        report: &Report,
    ) -> Result<Option<ProjectionManifest>> {
        if !self.projections_allowed(report) {
            return Ok(None);
        }
        let job = self.initial_namespace(report).await?;
        match job.get(GENERATION_FIELD) {
            None => Ok(None),
            Some(value) => {
                let generation = job.get_i64("generation").map_err(unavailable)?;
                if value.as_i64() != Some(generation)
                    || job.get_document(MANIFEST_FIELD).is_err()
                    || job.contains_key(INVALIDATED_FIELD)
                {
                    return Err(MetricsError::Unavailable);
                }
                if !job.contains_key("kind") {
                    // Ordinary reports also need full source membership once
                    // independently persisted aggregates can outlive a row.
                    self.retained_counts(report, &job).await?;
                }
                let manifest = ProjectionManifest::parse(
                    generation,
                    job.get_document(MANIFEST_FIELD).map_err(unavailable)?,
                )?;
                Ok(Some(manifest))
            }
        }
    }
}
