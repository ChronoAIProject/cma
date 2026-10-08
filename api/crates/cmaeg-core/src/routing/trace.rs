//! The bounded record of how a routing decision was reached.
use crate::{
    activity::RouteResolution,
    ids::{ConsumerId, RuleId},
};

/// The most steps one trace keeps.
pub const MAX_TRACE_STEPS: usize = 16;

/// At most 16 steps; never sealed, never sent to providers; feeds the
/// console's event detail. Steps hold only identifiers and fixed words, so a
/// trace never carries message text, event data or user tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trace {
    steps: Vec<TraceStep>,
    truncated: bool,
    selected_resolution: Option<RouteResolution>,
}

/// One step of a routing decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceStep {
    /// The precedence stage that decided: `interaction`, `agent_event`,
    /// `addressing`, `rules`, `default` or `kill_switch`.
    Precedence(&'static str),
    /// A rule was considered. `failed_on` names the first match list that
    /// refused the event; an inactive rule was skipped untried.
    RuleTried {
        rule_id: RuleId,
        matched: bool,
        inactive: bool,
        failed_on: Option<&'static str>,
    },
    /// A selected consumer was left out: `pending` or `ignores_actor_kind`.
    Skipped {
        consumer: ConsumerId,
        reason: &'static str,
    },
    /// Explicit addressing was read: its form (`mention`, `command` or
    /// `event_field`) and how many consumers it named.
    Addressing { form: &'static str, tokens: u8 },
    /// What the kill switch made of the shadow decision: `legacy` or `closed`.
    Shadow {
        resolution: RouteResolution,
        outcome: &'static str,
    },
}

impl Trace {
    /// Record a step; beyond [`MAX_TRACE_STEPS`] the step is dropped and the
    /// trace is marked truncated, so a 32-rule policy cannot grow it.
    pub(crate) fn push(&mut self, step: TraceStep) {
        if self.steps.len() < MAX_TRACE_STEPS {
            self.steps.push(step);
        } else {
            self.truncated = true;
        }
    }

    /// The deciding stage, including a refusal or kill-switch shadow. Kept
    /// separately so truncating diagnostic steps cannot lose classification.
    pub fn selected_resolution(&self) -> Option<RouteResolution> {
        self.selected_resolution
    }

    pub(crate) fn select(&mut self, resolution: RouteResolution) {
        self.selected_resolution = Some(resolution);
    }

    pub fn steps(&self) -> &[TraceStep] {
        &self.steps
    }

    /// Whether steps were dropped at the bound.
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}
