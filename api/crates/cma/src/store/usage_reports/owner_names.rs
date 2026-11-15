//! Public names stay current without becoming part of immutable usage evidence.
use super::*;
use layer2_core::sandbox_metrics::{public_option_name, Owner};

impl ReportingStore {
    pub(super) async fn hydrate_owner_names(&self, owners: Vec<&mut Owner>) -> Result<()> {
        if owners.is_empty() {
            return Ok(());
        }
        let _timer = self.time_read("report_owner_names")?;
        let subjects: Vec<_> = owners.iter().map(|owner| owner.subject.clone()).collect();
        // Resolve only this already-authorized page through the reporting pool.
        // The shared directory reader caps the lookup at 200 public identities.
        let names = tokio::time::timeout(
            DB_DEADLINE,
            super::super::directory::public_display_names(&self.client, self.db.name(), &subjects),
        )
        .await
        .map_err(|_| unavailable("report owner name lookup exceeded its deadline"))?
        .map_err(|_| unavailable("report owner names could not be loaded"))?;
        for owner in owners {
            // A deleted or no-longer-public name must clear any cached label.
            owner.display_name = public_option_name(
                &owner.subject,
                names.get(&owner.subject).map(String::as_str),
            )
            .map(str::to_owned);
        }
        Ok(())
    }
}
