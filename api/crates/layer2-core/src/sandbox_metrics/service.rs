mod initial;
mod view;
use super::*;
use crate::{Caller, Capability};
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait MetricsStore: Send + Sync {
    /// Verify the exact complete Ready namespace and return its writer generation.
    /// This observation alone does not authorize a report or any child read.
    async fn initial_snapshot_generation(&self, _report: &Report) -> Result<u64> {
        Err(MetricsError::Unavailable)
    }
    async fn latest(
        &self,
        _scope: &Scope,
        _selection: &ViewSelection,
        _now: u64,
    ) -> Result<ReportView> {
        Err(MetricsError::ViewDisabled)
    }
    /// Recheck immutable retained namespace completeness and expiry after child reads.
    async fn verify_snapshot(&self, _report: &Report) -> Result<()> {
        Ok(())
    }

    fn check_read_budget(&self) -> Result<()> {
        Ok(())
    }
    async fn owner_allocations(
        &self,
        _report: &Report,
        _query: OwnerAllocationQuery,
        _owner_context: OwnerContextAccess,
    ) -> Result<OwnerAllocations> {
        Err(MetricsError::Unavailable)
    }
    async fn options(
        &self,
        _scope: &Scope,
        _query: OptionQuery,
        _owner_context: OwnerContextAccess,
    ) -> Result<FilterOptions> {
        Err(MetricsError::Unavailable)
    }
    async fn resolve_agent(&self, scope: &Scope, id: &str) -> Result<String>;
    async fn authorize_sandbox(&self, scope: &Scope, id: &str) -> Result<()>;
    async fn submit(&self, scope: Scope, query: ReportQuery, now: u64) -> Result<Report>;
    async fn report(&self, scope: &Scope, id: &str, now: u64) -> Result<Report>;
    /// Check retained snapshot membership against current authoritative owners.
    /// A stale owner projection invalidates the whole report, including charts.
    async fn reauthorize(&self, report: &Report) -> Result<()>;
    async fn page(&self, report: &Report, query: PageQuery) -> Result<Rows>;
    async fn detail(&self, report: &Report, id: &str) -> Result<Row>;
    async fn series(&self, report: &Report, query: SeriesQuery) -> Result<Series>;
    async fn owners(&self, _report: &Report, _query: OwnerQuery) -> Result<Owners> {
        Err(MetricsError::Unavailable)
    }
}

pub struct MetricsService {
    store: Arc<dyn MetricsStore>,
}
impl MetricsService {
    pub async fn owner_allocations(
        &self,
        caller: &Caller,
        id: &str,
        query: OwnerAllocationQuery,
        now: u64,
    ) -> Result<OwnerAllocations> {
        authorize(caller, true)?;
        query.validate()?;
        self.store.check_read_budget()?;
        let report = self.ready(caller, true, id, now).await?;
        let value = self
            .store
            .owner_allocations(
                &report,
                query,
                OwnerContextAccess::from_authorization(&caller.authorization),
            )
            .await?;
        self.store.verify_snapshot(&report).await?;
        Ok(value)
    }
    pub async fn options(&self, caller: &Caller, query: OptionQuery) -> Result<FilterOptions> {
        let scope = authorize(caller, true)?;
        query.validate()?;
        self.store
            .options(
                &scope,
                query,
                OwnerContextAccess::from_authorization(&caller.authorization),
            )
            .await
    }
    pub fn new(store: Arc<dyn MetricsStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }
    pub async fn submit(
        &self,
        caller: &Caller,
        admin: bool,
        input: ReportInput,
        now: u64,
    ) -> Result<Report> {
        let scope = authorize(caller, admin)?;
        let mut query = input.validate(now)?;
        self.select_resources(&scope, &mut query).await?;
        let report = self.store.submit(scope.clone(), query, now).await?;
        if report.scope != scope {
            return Err(MetricsError::NotFound);
        }
        self.store.reauthorize(&report).await?;
        Ok(report)
    }
    async fn select_resources(&self, scope: &Scope, query: &mut ReportQuery) -> Result<()> {
        if !scope.admin && query.filters.owner.is_some() {
            return Err(MetricsError::Invalid(
                "Owner parameters are forbidden on owner routes.".into(),
            ));
        }
        if let Some(agent) = &query.filters.agent_id {
            let sandbox = self.store.resolve_agent(scope, agent).await?;
            if query
                .filters
                .sandbox_id
                .as_ref()
                .is_some_and(|id| id != &sandbox)
            {
                return Err(MetricsError::Invalid(
                    "Agent and Sandbox selections disagree.".into(),
                ));
            }
            query.filters.sandbox_id = Some(sandbox);
        }
        if let Some(id) = &query.filters.sandbox_id {
            self.store.authorize_sandbox(scope, id).await?;
        }
        Ok(())
    }
    pub async fn report(&self, caller: &Caller, admin: bool, id: &str, now: u64) -> Result<Report> {
        let scope = authorize(caller, admin)?;
        if !query::opaque_id(id, "umr_") {
            return Err(MetricsError::NotFound);
        }
        let report = self.store.report(&scope, id, now).await?;
        if report.scope != scope {
            return Err(MetricsError::NotFound);
        }
        if report.expires_at <= now {
            return Err(MetricsError::Expired);
        }
        self.store.reauthorize(&report).await?;
        Ok(report)
    }
    pub async fn page(
        &self,
        caller: &Caller,
        admin: bool,
        id: &str,
        query: PageQuery,
        now: u64,
    ) -> Result<Rows> {
        authorize(caller, admin)?;
        query.validate(admin)?;
        let report = self.ready(caller, admin, id, now).await?;
        let value = self.store.page(&report, query).await?;
        self.store.verify_snapshot(&report).await?;
        Ok(value)
    }
    pub async fn detail(
        &self,
        caller: &Caller,
        admin: bool,
        report: &str,
        id: &str,
        now: u64,
    ) -> Result<Row> {
        let report = self.ready(caller, admin, report, now).await?;
        if !query::opaque_id(id, "sbx_") {
            return Err(MetricsError::NotFound);
        }
        self.store.authorize_sandbox(&report.scope, id).await?;
        let value = self.store.detail(&report, id).await?;
        self.store.verify_snapshot(&report).await?;
        Ok(value)
    }
    pub async fn series(
        &self,
        caller: &Caller,
        admin: bool,
        id: &str,
        query: SeriesQuery,
        now: u64,
    ) -> Result<Series> {
        authorize(caller, admin)?;
        query.validate(admin)?;
        let report = self.ready(caller, admin, id, now).await?;
        if query.group_by == GroupBy::Sandbox {
            if let Some(id) = &query.series_id {
                self.store.authorize_sandbox(&report.scope, id).await?;
            }
        }
        let value = self.store.series(&report, query).await?;
        self.store.verify_snapshot(&report).await?;
        Ok(value)
    }
    pub async fn owners(
        &self,
        caller: &Caller,
        id: &str,
        query: OwnerQuery,
        now: u64,
    ) -> Result<Owners> {
        authorize(caller, true)?;
        query.validate()?;
        let report = self.ready(caller, true, id, now).await?;
        let value = self.store.owners(&report, query).await?;
        self.store.verify_snapshot(&report).await?;
        Ok(value)
    }
    async fn ready(&self, caller: &Caller, admin: bool, id: &str, now: u64) -> Result<Report> {
        let report = self.report(caller, admin, id, now).await?;
        if report.status != ReportStatus::Ready {
            return Err(MetricsError::Unavailable);
        }
        Ok(report)
    }
}
pub fn authorize(caller: &Caller, admin: bool) -> Result<Scope> {
    let allowed = if admin {
        caller.authorization.allows(Capability::ReadAllSandboxes)
    } else {
        caller.authorization.allows(Capability::UseOwnSandboxes)
            || caller.authorization.allows(Capability::LaunchTriggers)
    };
    if !allowed {
        return Err(MetricsError::Forbidden);
    }
    Ok(Scope {
        principal: caller.subject.to_string(),
        admin,
    })
}
pub fn query_hash(scope: &Scope, query: &ReportQuery, minute: u64) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(ALGORITHM_VERSION, scope, query, minute))
        .map_err(|_| MetricsError::Unavailable)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
