//! How an event was routed, as its activity entry keeps it: the decision
//! bound into the activity (#989) and the resolver's bounded trace (#991).
//!
//! Both hold only identifiers and fixed words. A trace never carries
//! message text, event data or the tokens a sender typed, and target names
//! are cut like every display name.
use cmaeg_core::{
    activity::{ActivityRouting, RouteResolution},
    channel::SourceId,
    ids::ConsumerId,
    routing::{Trace, TraceStep},
};
use serde::Serialize;

use crate::console::mask;

/// What each trace step and each target is counted as, beyond its strings.
const PART_BYTES: usize = 48;

/// The routing decision an event carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoutingSummary {
    /// The slot the event arrived on; `agent` for a hand-off.
    pub source_id: SourceId,
    pub resolution: RouteResolution,
    /// The rule that selected the targets, when a rule did.
    pub rule_id: Option<String>,
    /// In order; the first is the primary consumer.
    pub targets: Vec<RouteTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteTarget {
    pub id: ConsumerId,
    pub name: String,
}

impl RoutingSummary {
    pub fn of(routing: &ActivityRouting) -> Self {
        Self {
            source_id: routing.source_id.clone(),
            resolution: routing.resolution,
            rule_id: routing.rule_id.as_ref().map(|id| id.as_str().to_owned()),
            targets: routing
                .targets
                .iter()
                .map(|target| RouteTarget {
                    id: target.id.clone(),
                    name: mask::display_name(target.name.as_str()),
                })
                .collect(),
        }
    }

    /// The bytes it counts against the activity budget.
    pub fn weight(&self) -> usize {
        self.source_id.as_str().len()
            + self.rule_id.as_ref().map_or(0, String::len)
            + self
                .targets
                .iter()
                .map(|target| PART_BYTES + target.id.as_str().len() + target.name.len())
                .sum::<usize>()
    }
}

/// The resolver's trace: at most 16 steps, marked when steps were dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TraceView {
    pub steps: Vec<TraceStepView>,
    pub truncated: bool,
}

/// One step of a routing decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum TraceStepView {
    /// The precedence stage that decided: `interaction`, `agent_event`,
    /// `addressing`, `rules`, `default` or `kill_switch`.
    Precedence { stage: &'static str },
    /// A rule was considered; `failed_on` names the first match list that
    /// refused the event, and an inactive rule was skipped untried.
    Rule {
        rule_id: String,
        matched: bool,
        inactive: bool,
        failed_on: Option<&'static str>,
    },
    /// A selected consumer was left out: `pending` or `ignores_actor_kind`.
    Skipped {
        consumer: ConsumerId,
        reason: &'static str,
    },
    /// Explicit addressing: its form and how many consumers it named.
    Addressing { form: &'static str, tokens: u8 },
    /// The kill switch's shadow decision: `legacy` or `closed`.
    Shadow {
        resolution: RouteResolution,
        outcome: &'static str,
    },
}

impl TraceView {
    pub fn of(trace: &Trace) -> Self {
        Self {
            steps: trace.steps().iter().map(TraceStepView::of).collect(),
            truncated: trace.truncated(),
        }
    }

    /// The bytes it counts against the activity budget.
    pub fn weight(&self) -> usize {
        self.steps
            .iter()
            .map(|step| {
                PART_BYTES
                    + match step {
                        TraceStepView::Rule { rule_id, .. } => rule_id.len(),
                        TraceStepView::Skipped { consumer, .. } => consumer.as_str().len(),
                        _ => 0,
                    }
            })
            .sum()
    }
}

impl TraceStepView {
    fn of(step: &TraceStep) -> Self {
        match step {
            TraceStep::Precedence(stage) => Self::Precedence { stage },
            TraceStep::RuleTried {
                rule_id,
                matched,
                inactive,
                failed_on,
            } => Self::Rule {
                rule_id: rule_id.as_str().to_owned(),
                matched: *matched,
                inactive: *inactive,
                failed_on: *failed_on,
            },
            TraceStep::Skipped { consumer, reason } => Self::Skipped {
                consumer: consumer.clone(),
                reason,
            },
            TraceStep::Addressing { form, tokens } => Self::Addressing {
                form,
                tokens: *tokens,
            },
            TraceStep::Shadow {
                resolution,
                outcome,
            } => Self::Shadow {
                resolution: *resolution,
                outcome,
            },
        }
    }
}
