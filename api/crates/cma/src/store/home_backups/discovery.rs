//! Bounded discovery is independent of catalog completeness and never writes Sandbox.
use super::*;
use layer2_core::sandbox::store::{Page, PageRequest, MAX_PAGE_SIZE};
use layer2_core::sandbox::{Sandbox, SandboxStatus};
use layer3_adapter::HomeVolumeMode;

#[async_trait::async_trait]
impl HomeBackupDiscoveryStore for MongoHomeBackupStore {
    async fn backfill_schedules(
        &self,
        page: &PageRequest,
        now: &str,
    ) -> Result<Page<HomeBackupCoverage>, HomeBackupError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(HomeBackupError::Invalid("invalid discovery page"));
        }
        if layer2_core::seconds_from_rfc3339(now).is_none() {
            return Err(HomeBackupError::Invalid("noncanonical discovery time"));
        }
        let parents = super::super::durability::majority_collection::<Document>(
            &self.client.database(&self.schedules.namespace().db),
            "sandboxes",
        );
        let mut filter = doc! {};
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! {"$gt": cursor});
        }
        let mut cursor = parents
            .find(super::super::work_schema::query(filter))
            .sort(doc! {"_id": 1})
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(failed)?;
        let mut values = Vec::new();
        while let Some(row) = cursor.try_next().await.map_err(failed)? {
            values.push(
                super::super::sandboxes::read_sandbox(&row)
                    .map_err(|_| HomeBackupError::Invalid("invalid retained home authority"))?,
            );
        }
        let next = if values.len() > usize::from(page.limit) {
            values.pop();
            values.last().map(|sandbox| sandbox.id.to_string())
        } else {
            None
        };
        let mut items = Vec::new();
        for sandbox in values {
            let Some(home) = sandbox
                .lifecycle
                .home
                .as_deref()
                .filter(|home| !home.released)
            else {
                continue;
            };
            let mut coverage = HomeBackupCoverage {
                sandbox_id: sandbox.id.to_string(),
                schedule_present: false,
                latest_ready_at: None,
            };
            if let Some(seed) = schedule_for(&sandbox, now)? {
                if home.request.policy.is_native_vct() {
                    if matches!(
                        sandbox.status,
                        SandboxStatus::Retiring | SandboxStatus::Retired
                    ) {
                        self.retire_schedule(&seed).await?;
                    } else {
                        self.ensure_schedule(&seed).await?;
                    }
                }
                // Unsupported legacy homes stay visible in the denominator;
                // discovery does not implement or touch their provider path.
                let schedule = self.get_schedule(&seed.id).await?;
                if schedule
                    .as_ref()
                    .is_some_and(|value| value.home != seed.home)
                {
                    return Err(HomeBackupError::Conflict);
                }
                coverage.schedule_present = schedule.is_some();
                let row = self.backups.find_one(doc! {
                    "sandbox_id": &seed.id,
                    "state": "ready",
                    "source.pvc_uid": &seed.home.home_source_id,
                    "source.home_identity": &seed.home.home_identity,
                    "$expr": {"$gt": [
                        {"$dateFromString": {"dateString": "$retention.expires_at", "onError": null, "onNull": null}},
                        {"$dateFromString": {"dateString": now}},
                    ]},
                }).sort(doc! {"cut.ready_at": -1, "_id": -1}).await.map_err(failed)?;
                coverage.latest_ready_at = row
                    .map(read_backup)
                    .transpose()
                    .map_err(malformed)?
                    .and_then(|value| value.cut.ready_at);
            }
            items.push(coverage);
        }
        Ok(Page { items, next })
    }
}

fn schedule_for(
    sandbox: &Sandbox,
    now: &str,
) -> Result<Option<HomeBackupSchedule>, HomeBackupError> {
    let Some(home) = sandbox.lifecycle.home.as_deref() else {
        return Ok(None);
    };
    let Some(attachment) = home.attachment() else {
        return Ok(None);
    };
    let identity = attachment
        .identity()
        .map_err(|_| HomeBackupError::Invalid("invalid retained home identity"))?;
    Ok(Some(HomeBackupSchedule {
        id: sandbox.id.to_string(),
        home: HomeBackupHome {
            mode: match attachment.request.policy.mode {
                HomeVolumeMode::LegacySnapshot => "legacy_snapshot",
                HomeVolumeMode::NativeVctEmptyV1 => "native_vct_empty_v1",
                HomeVolumeMode::NativeVctFullHomeV2 => "native_vct_full_home_v2",
            }
            .into(),
            home_source_id: attachment.receipt.storage_generation,
            home_identity: identity,
            source_reference: Some(attachment.receipt.storage_reference),
        },
        state: HomeBackupScheduleState::Active,
        next_due_at: now.into(),
        last_ready_at: None,
        last_ready_backup_id: None,
        lease: None,
        next_fence: 1,
        in_flight: None,
        revision: 1,
    }))
}
