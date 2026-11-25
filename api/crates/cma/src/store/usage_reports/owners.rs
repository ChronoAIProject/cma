use super::*;
use layer2_core::sandbox_metrics::{OwnerQuery, Owners, Report};

impl ReportingStore {
    pub(super) async fn read_owners(&self, report: &Report, query: OwnerQuery) -> Result<Owners> {
        let limit = cma_am_reporting::validate_owner_page(report, &query)?;
        let _timer = self.time_read("report_owner_search")?;
        let mut page = cma_am_reporting::read_owner_page(
            self.collection(ROWS),
            &self.signer,
            report,
            query,
            limit,
            DB_DEADLINE,
        )
        .await?;
        self.hydrate_owner_names(page.items.iter_mut().collect())
            .await?;
        Ok(page)
    }
}
