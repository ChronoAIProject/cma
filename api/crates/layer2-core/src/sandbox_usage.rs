//! Allocation metering at the durable Sandbox lifecycle boundary.
//!
//! CPU and memory accrue while a nonterminal Sandbox has a runtime bound.
//! A retained home accrues storage even while its runtime is paused or failed.
//! These are allocated resource-time quantities, not sampled CPU utilization,
//! RSS or filesystem bytes used.

use serde::{Deserialize, Serialize};

use crate::{
    sandbox::{Sandbox, SandboxStatus},
    seconds_from_rfc3339, SandboxId, SubjectId,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub running: bool,
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub disk_gib: u32,
}

impl Allocation {
    pub fn from_sandbox(sandbox: &Sandbox) -> Self {
        let running = sandbox.runtime.is_some()
            && !matches!(
                sandbox.status,
                SandboxStatus::Failed | SandboxStatus::Retired
            );
        Self {
            running,
            cpu_millis: if running {
                sandbox.profile.content.cpu_millis
            } else {
                0
            },
            memory_mib: if running {
                sandbox.profile.content.memory_mib
            } else {
                0
            },
            disk_gib: sandbox
                .lifecycle
                .home
                .as_ref()
                .filter(|home| home.receipt.is_some() && !home.released)
                .map_or(0, |home| home.request.policy.capacity_gib),
        }
    }

    fn without_compute(self) -> Self {
        Self {
            running: false,
            cpu_millis: 0,
            memory_mib: 0,
            ..self
        }
    }

    pub fn usage(&self, seconds: u64) -> Result<UsageTotals, UsageError> {
        let product = |quantity: u32| {
            u64::from(quantity)
                .checked_mul(seconds)
                .ok_or(UsageError::Overflow)
        };
        Ok(UsageTotals {
            uptime_seconds: if self.running { seconds } else { 0 },
            cpu_millis_seconds: product(self.cpu_millis)?,
            memory_mib_seconds: product(self.memory_mib)?,
            disk_gib_seconds: product(self.disk_gib)?,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageTotals {
    pub uptime_seconds: u64,
    pub cpu_millis_seconds: u64,
    pub memory_mib_seconds: u64,
    pub disk_gib_seconds: u64,
}

impl UsageTotals {
    pub fn checked_add(&self, other: &Self) -> Result<Self, UsageError> {
        let add = |a: u64, b: u64| a.checked_add(b).ok_or(UsageError::Overflow);
        Ok(Self {
            uptime_seconds: add(self.uptime_seconds, other.uptime_seconds)?,
            cpu_millis_seconds: add(self.cpu_millis_seconds, other.cpu_millis_seconds)?,
            memory_mib_seconds: add(self.memory_mib_seconds, other.memory_mib_seconds)?,
            disk_gib_seconds: add(self.disk_gib_seconds, other.disk_gib_seconds)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageEventKind {
    Created,
    Started,
    Paused,
    Resumed,
    Terminated,
    ResourcesChanged,
    StateChanged,
    Checkpoint,
    /// Existing Sandbox: only observations from this point are accounted.
    Baseline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxUsage {
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub name: String,
    pub status: String,
    pub allocation: Allocation,
    pub created_at: u64,
    pub tracking_started_at: u64,
    pub last_observed_at: u64,
    pub source_seq: u64,
    pub source_revision: u64,
    pub revision: u64,
    pub has_started: bool,
    pub complete_lifecycle: bool,
    pub totals: UsageTotals,
}

impl SandboxUsage {
    /// Legacy terminal rows can retain compute allocation until the next checkpoint.
    pub fn current_allocation(&self) -> Allocation {
        if matches!(self.status.as_str(), "failed" | "retired") {
            self.allocation.clone().without_compute()
        } else {
            self.allocation.clone()
        }
    }

    /// A live tail is projected at read time; reads never advance the ledger.
    pub fn totals_at(&self, as_of: u64) -> Result<UsageTotals, UsageError> {
        self.totals.checked_add(
            &self.current_allocation().usage(
                as_of
                    .checked_sub(self.last_observed_at)
                    .ok_or(UsageError::InvalidTime)?,
            )?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageEvent {
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub revision: u64,
    pub source_seq: u64,
    pub kind: UsageEventKind,
    pub started_at: u64,
    pub ended_at: u64,
    /// Allocation during [started_at, ended_at), before the transition.
    pub allocation: Allocation,
    pub next_allocation: Allocation,
    pub status: String,
    pub quantity: UsageTotals,
}

impl UsageEvent {
    pub fn transaction_id(&self) -> String {
        format!("cma-usage:{}:{}", self.sandbox_id, self.revision)
    }

    pub fn within(&self, from: u64, to: u64) -> Result<UsageTotals, UsageError> {
        if from > to {
            return Err(UsageError::InvalidTime);
        }
        self.allocation.usage(
            self.ended_at
                .min(to)
                .saturating_sub(self.started_at.max(from)),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageError {
    #[error("invalid usage timestamp")]
    InvalidTime,
    #[error("usage quantity overflow")]
    Overflow,
    #[error("usage observation conflicts with recorded ownership or sequence")]
    Conflict,
}

/// Pure lifecycle middleware. Persistence commits the returned state and event
/// in the same transaction as the source lifecycle change. A periodic checkpoint
/// closes a stable billing interval without modifying the Sandbox itself.
pub fn observe(
    previous: Option<&SandboxUsage>,
    sandbox: &Sandbox,
    source_seq: u64,
    at: &str,
    checkpoint: bool,
) -> Result<Option<(SandboxUsage, UsageEvent)>, UsageError> {
    observe_inner(previous, sandbox, source_seq, at, checkpoint, false)
}

/// Close observed zero-allocation time only while a funded window needs proof.
/// Callers must use an actual observation time, never a projected future cutoff.
pub fn observe_funded_checkpoint(
    previous: Option<&SandboxUsage>,
    sandbox: &Sandbox,
    source_seq: u64,
    at: &str,
) -> Result<Option<(SandboxUsage, UsageEvent)>, UsageError> {
    observe_inner(previous, sandbox, source_seq, at, true, true)
}

/// Retained evidence includes observed zero allocation for every owner.
pub fn observe_capture_checkpoint(
    previous: Option<&SandboxUsage>,
    sandbox: &Sandbox,
    source_seq: u64,
    at: &str,
) -> Result<Option<(SandboxUsage, UsageEvent)>, UsageError> {
    observe_inner(previous, sandbox, source_seq, at, true, true)
}

fn observe_inner(
    previous: Option<&SandboxUsage>,
    sandbox: &Sandbox,
    source_seq: u64,
    at: &str,
    checkpoint: bool,
    include_zero: bool,
) -> Result<Option<(SandboxUsage, UsageEvent)>, UsageError> {
    let observed = timestamp(at)?;
    let created_at = timestamp(&sandbox.created_at)?;
    let mut allocation = Allocation::from_sandbox(sandbox);
    let status = sandbox.status.name().to_string();
    let Some(previous) = previous else {
        if observed < created_at {
            return Err(UsageError::InvalidTime);
        }
        let complete = source_seq == 1 && observed == created_at;
        let state = SandboxUsage {
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            name: sandbox.name.clone(),
            status: status.clone(),
            allocation: allocation.clone(),
            created_at,
            tracking_started_at: observed,
            last_observed_at: observed,
            source_seq,
            source_revision: sandbox.revision,
            revision: 1,
            has_started: allocation.running,
            complete_lifecycle: complete,
            totals: UsageTotals::default(),
        };
        let event = UsageEvent {
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            revision: 1,
            source_seq,
            kind: if complete {
                UsageEventKind::Created
            } else {
                UsageEventKind::Baseline
            },
            started_at: observed,
            ended_at: observed,
            allocation: Allocation::default(),
            next_allocation: allocation,
            status,
            quantity: UsageTotals::default(),
        };
        return Ok(Some((state, event)));
    };
    if previous.sandbox_id != sandbox.id
        || previous.owner != sandbox.owner
        || source_seq < previous.source_seq
        || sandbox.revision < previous.source_revision
    {
        return Err(UsageError::Conflict);
    }
    // Retirement cannot restart compute that terminal failure already stopped.
    if sandbox.status == SandboxStatus::Retiring
        && (matches!(previous.status.as_str(), "failed" | "retired")
            || (previous.status == "retiring" && !previous.allocation.running))
    {
        allocation = allocation.without_compute();
    }
    let previous_allocation = previous.current_allocation();
    let terminal_correction = previous_allocation != previous.allocation
        && previous_allocation == allocation
        && previous.status == status
        && previous.name == sandbox.name;
    let unchanged = previous.allocation == allocation
        && previous.status == status
        && previous.name == sandbox.name;
    if source_seq == previous.source_seq
        && sandbox.revision == previous.source_revision
        && !unchanged
        && !terminal_correction
    {
        return Err(UsageError::Conflict);
    }
    // Concurrent callers may carry a timestamp sampled before a newer committed
    // transition. Clamp to that transition; never manufacture negative usage.
    let ended_at = observed.max(previous.last_observed_at);
    let source_advanced =
        source_seq != previous.source_seq || sandbox.revision != previous.source_revision;
    if unchanged
        && !source_advanced
        && (!checkpoint
            || ended_at == previous.last_observed_at
            || (!include_zero && previous.allocation == Allocation::default()))
    {
        return Ok(None);
    }
    let kind = if status == "retired" {
        UsageEventKind::Terminated
    } else if previous.allocation.running && !allocation.running {
        if matches!(status.as_str(), "pausing" | "paused") {
            UsageEventKind::Paused
        } else {
            UsageEventKind::Terminated
        }
    } else if !previous.allocation.running && allocation.running {
        if previous.has_started {
            UsageEventKind::Resumed
        } else {
            UsageEventKind::Started
        }
    } else if previous.allocation != allocation {
        UsageEventKind::ResourcesChanged
    } else if unchanged {
        UsageEventKind::Checkpoint
    } else {
        UsageEventKind::StateChanged
    };
    let quantity = previous_allocation.usage(ended_at - previous.last_observed_at)?;
    let revision = previous
        .revision
        .checked_add(1)
        .ok_or(UsageError::Overflow)?;
    let event = UsageEvent {
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        revision,
        source_seq,
        kind,
        started_at: previous.last_observed_at,
        ended_at,
        allocation: previous_allocation,
        next_allocation: allocation.clone(),
        status: status.clone(),
        quantity: quantity.clone(),
    };
    let state = SandboxUsage {
        name: sandbox.name.clone(),
        status,
        allocation,
        last_observed_at: ended_at,
        source_seq,
        source_revision: sandbox.revision,
        revision,
        has_started: previous.has_started || event.next_allocation.running,
        totals: previous.totals.checked_add(&quantity)?,
        ..previous.clone()
    };
    Ok(Some((state, event)))
}

/// An older writer changed the aggregate without advancing its usage ledger.
/// Preserve confirmed totals, exclude the uncertain tail and resume coverage.
pub fn rebaseline(
    previous: &SandboxUsage,
    sandbox: &Sandbox,
    source_seq: u64,
    at: &str,
) -> Result<Option<(SandboxUsage, UsageEvent)>, UsageError> {
    let mut anchor = previous.clone();
    anchor.last_observed_at = timestamp(at)?.max(previous.last_observed_at);
    Ok(
        observe(Some(&anchor), sandbox, source_seq, at, true)?.map(|(mut state, mut event)| {
            state.complete_lifecycle = false;
            event.kind = UsageEventKind::Baseline;
            (state, event)
        }),
    )
}

pub fn timestamp(value: &str) -> Result<u64, UsageError> {
    seconds_from_rfc3339(value)
        .filter(|value| *value > 0)
        .ok_or(UsageError::InvalidTime)
}
