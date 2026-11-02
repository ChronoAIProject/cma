//! Retained allocation intervals replay through the real owner/operator service.
use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::sandbox::store::{Page, PageRequest, SandboxStoreError};
use layer2_core::sandbox_usage::{Allocation, SandboxUsage, UsageEvent, UsageEventKind};
use layer2_core::sandbox_usage_service::{SandboxUsageService, SandboxUsageStore, UsageSummary};
use layer2_core::{Authorization, Capability, SandboxId, SubjectId};

use super::Scenario;

struct Store {
    unavailable: bool,
}

fn allocation() -> Allocation {
    Allocation {
        running: true,
        cpu_millis: 2000,
        memory_mib: 1024,
        disk_gib: 10,
    }
}

fn retained() -> SandboxUsage {
    SandboxUsage {
        sandbox_id: SandboxId("sbx_usage_contract".into()),
        owner: SubjectId::new("owner"),
        name: "Retained usage".into(),
        status: "deleted".into(),
        allocation: Allocation::default(),
        created_at: 100,
        tracking_started_at: 100,
        last_observed_at: 3700,
        source_seq: 4,
        source_revision: 4,
        revision: 2,
        has_started: true,
        complete_lifecycle: true,
        totals: allocation().usage(3600).unwrap(),
    }
}

impl Store {
    fn check(&self) -> Result<(), SandboxStoreError> {
        if self.unavailable {
            Err(SandboxStoreError::Unavailable(
                "contract store unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl SandboxUsageStore for Store {
    async fn usage(&self, id: &SandboxId) -> Result<Option<SandboxUsage>, SandboxStoreError> {
        self.check()?;
        Ok((id == &retained().sandbox_id).then(retained))
    }
    async fn usage_page(
        &self,
        owner: Option<&SubjectId>,
        _: &PageRequest,
    ) -> Result<Page<SandboxUsage>, SandboxStoreError> {
        self.check()?;
        Ok(Page {
            items: if owner.is_none_or(|owner| owner == &retained().owner) {
                vec![retained()]
            } else {
                vec![]
            },
            next: None,
        })
    }
    async fn usage_summary(
        &self,
        owner: Option<&SubjectId>,
        _: u64,
    ) -> Result<UsageSummary, SandboxStoreError> {
        self.check()?;
        let included = owner.is_none_or(|owner| owner == &retained().owner);
        Ok(UsageSummary {
            sandbox_count: u64::from(included),
            running_count: 0,
            incomplete_lifecycle_count: 0,
            tracking_started_at: included.then_some(100),
            totals: if included {
                retained().totals
            } else {
                Default::default()
            },
        })
    }
    async fn usage_events(
        &self,
        _: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<UsageEvent>, SandboxStoreError> {
        self.check()?;
        if page.cursor.as_deref() == Some("invalid") {
            return Err(SandboxStoreError::InvalidPage);
        }
        Ok(Page {
            items: vec![UsageEvent {
                sandbox_id: retained().sandbox_id,
                owner: retained().owner,
                revision: 2,
                source_seq: 4,
                kind: UsageEventKind::Terminated,
                started_at: 100,
                ended_at: 3700,
                allocation: allocation(),
                next_allocation: Allocation::default(),
                status: "deleted".into(),
                quantity: retained().totals,
            }],
            next: None,
        })
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let (subject, capabilities) = match name {
        "usage_owner" | "usage_unavailable" => (Some("owner"), vec![Capability::UseOwnSandboxes]),
        "usage_other_owner" => (Some("other-owner"), vec![Capability::UseOwnSandboxes]),
        "usage_operator" | "usage_operator_unavailable" => {
            (Some("operator"), vec![Capability::ReadAllSandboxes])
        }
        "usage_anonymous" => (None, vec![]),
        "usage_forbidden" => (Some("owner"), vec![]),
        _ => return None,
    };
    Some(Scenario {
        router: super::super::sandbox_usage::sandbox_usage_router(SandboxUsageService::new(
            Arc::new(Store {
                unavailable: name.ends_with("unavailable"),
            }),
        )),
        subject,
        authorization: Authorization::new(capabilities),
        symbols: Default::default(),
    })
}
