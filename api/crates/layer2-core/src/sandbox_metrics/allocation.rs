//! Read-only projection from immutable allocated ledger intervals.
use super::{AllocatedPeriod, Range};
use crate::sandbox_usage::{Allocation, SandboxUsage, UsageEvent, UsageEventKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub from: u64,
    pub to: u64,
    pub running: Option<bool>,
    pub home: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    pub period: AllocatedPeriod,
    pub expectations: Vec<Expectation>,
}
pub fn period(state: &SandboxUsage, events: &[UsageEvent], range: Range) -> AllocatedPeriod {
    project(state, events, range).period
}
pub fn project(state: &SandboxUsage, events: &[UsageEvent], range: Range) -> Projection {
    let range = Range {
        from: range.from.max(state.created_at),
        to: range.to,
    };
    let mut out = AllocatedPeriod {
        complete: true,
        ..Default::default()
    };
    let mut expectations = Vec::new();
    let mut end = range.from;
    let mut revision = None;
    for event in events {
        if event.sandbox_id != state.sandbox_id || event.owner != state.owner {
            out.complete = false;
            continue;
        }
        let clipped = Range {
            from: event.started_at,
            to: event.ended_at,
        }
        .intersect(range);
        if clipped.seconds() == 0 {
            continue;
        }
        if event.kind == UsageEventKind::Baseline
            || clipped.from < end
            || revision.is_some_and(|r| event.revision != r + 1)
        {
            out.complete = false;
        }
        if clipped.from < end || event.kind == UsageEventKind::Baseline {
            continue;
        }
        out.unknown_seconds += clipped.from.saturating_sub(end);
        expect(&mut expectations, end, clipped.from, None);
        expect(
            &mut expectations,
            clipped.from,
            clipped.to,
            Some(&event.allocation),
        );
        accrue(&mut out, &event.allocation, clipped.seconds());
        end = clipped.to;
        revision = Some(event.revision);
    }
    let tail = Range {
        from: state.last_observed_at,
        to: range.to,
    }
    .intersect(range);
    if tail.seconds() > 0 && tail.from >= end && revision.is_none_or(|r| r == state.revision) {
        out.unknown_seconds += tail.from.saturating_sub(end);
        expect(&mut expectations, end, tail.from, None);
        expect(
            &mut expectations,
            tail.from,
            tail.to,
            Some(&state.allocation),
        );
        accrue(&mut out, &state.allocation, tail.seconds());
        out.projected_tail_seconds = tail.seconds();
        end = tail.to;
    }
    out.unknown_seconds += range.to.saturating_sub(end);
    out.complete &= out.unknown_seconds == 0
        && (range.from >= state.tracking_started_at || state.complete_lifecycle);
    expect(&mut expectations, end, range.to, None);
    Projection {
        period: out,
        expectations,
    }
}
fn expect(out: &mut Vec<Expectation>, from: u64, to: u64, allocation: Option<&Allocation>) {
    if from >= to {
        return;
    }
    let running = allocation.map(|a| a.running);
    let home = allocation.map(|a| a.disk_gib > 0);
    if let Some(last) = out.last_mut() {
        if last.to == from && last.running == running && last.home == home {
            last.to = to;
            return;
        }
    }
    out.push(Expectation {
        from,
        to,
        running,
        home,
    });
}
pub fn accrue(out: &mut AllocatedPeriod, allocation: &Allocation, seconds: u64) {
    out.disk_retained_seconds += if allocation.disk_gib > 0 {
        seconds as f64
    } else {
        0.0
    };
    out.uptime_seconds += if allocation.running {
        seconds as f64
    } else {
        0.0
    };
    out.cpu_core_hours += allocation.cpu_millis as f64 * seconds as f64 / 3_600_000.0;
    out.memory_gib_hours += allocation.memory_mib as f64 * seconds as f64 / 3_686_400.0;
    out.disk_gib_hours += allocation.disk_gib as f64 * seconds as f64 / 3600.0;
}
