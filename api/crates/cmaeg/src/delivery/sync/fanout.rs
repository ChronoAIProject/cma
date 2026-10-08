//! Bounded identity and terminal accounting for one opt-in multiplexed waiter.
use crate::tools::ConsumerSummary;
use cmaeg_core::{
    activity::ConsumerTarget,
    ids::{ConsumerId, ConsumerName},
};
use serde_json::{json, Value};

pub(super) struct Fanout {
    count: u8,
    remaining: u8,
    default: ConsumerSummary,
    targets: Vec<ConsumerTarget>,
    outcomes: Vec<Option<&'static str>>,
    configured: bool,
    started: bool,
}

pub(super) enum Emission {
    /// Terminals are already handled; a discarded message is not delivered.
    Suppressed(bool),
    Frame {
        name: &'static str,
        data: Value,
        final_data: Option<Value>,
    },
}

impl Fanout {
    pub fn new(count: u8, default: ConsumerSummary) -> Option<Self> {
        if !(1..=8).contains(&count) || !default.id.is_default() {
            return None;
        }
        let targets = if count == 1 {
            vec![ConsumerTarget {
                id: default.id.clone(),
                name: default.name.clone(),
            }]
        } else {
            Vec::new()
        };
        Some(Self {
            count,
            remaining: count,
            default,
            targets,
            outcomes: vec![None; usize::from(count)],
            configured: false,
            started: false,
        })
    }

    /// The registration API knows width, not order. Ingress pins the actual
    /// decision before launching any child; no completion-order fallback exists.
    pub fn set_targets(&mut self, targets: &[ConsumerTarget]) -> bool {
        if self.configured
            || self.started
            || targets.len() != usize::from(self.count)
            || targets
                .iter()
                .enumerate()
                .any(|(at, target)| targets[..at].iter().any(|prior| prior.id == target.id))
        {
            return false;
        }
        self.targets = targets.to_vec();
        self.configured = true;
        true
    }

    pub fn translate(
        &mut self,
        name: &'static str,
        data: &Value,
        terminal: bool,
    ) -> Result<Emission, ()> {
        let translated = match name {
            "cmaeg.text.delta" if !terminal => "cmaeg.consumer.delta",
            "cmaeg.message" if !terminal => "cmaeg.consumer.message",
            "cmaeg.completed" if terminal => "cmaeg.consumer.completed",
            "cmaeg.failed" if terminal => "cmaeg.consumer.failed",
            _ => return Err(()),
        };
        if self.targets.len() != usize::from(self.count) || !data.is_object() {
            return Err(());
        }
        let id = match data.get("consumer") {
            None => self.default.id.clone(),
            Some(consumer) => {
                let id = ConsumerId::parse(consumer.get("id").and_then(Value::as_str).ok_or(())?)
                    .map_err(|_| ())?;
                ConsumerName::parse(consumer.get("name").and_then(Value::as_str).ok_or(())?)
                    .map_err(|_| ())?;
                consumer.get("label").and_then(Value::as_str).ok_or(())?;
                id
            }
        };
        let at = self
            .targets
            .iter()
            .position(|target| target.id == id)
            .ok_or(())?;
        if self.outcomes[at].is_some() {
            return Ok(Emission::Suppressed(terminal));
        }
        self.started = true;
        let mut data = data.clone();
        if data.get("consumer").is_none() {
            data["consumer"] = json!(self.default);
        }
        let final_data = if terminal {
            self.outcomes[at] = Some(if name == "cmaeg.completed" {
                "completed"
            } else if data.get("refused") == Some(&Value::Bool(true)) {
                "refused"
            } else {
                "failed"
            });
            self.remaining -= 1;
            (self.remaining == 0).then(|| {
                let consumers: Vec<_> = self.targets.iter().zip(&self.outcomes).map(|(target, outcome)| {
                    json!({"id":target.id,"name":target.name,"outcome":outcome.expect("every expected consumer is terminal")})
                }).collect();
                json!({"event_id":data["event_id"],"consumers":consumers})
            })
        } else {
            None
        };
        Ok(Emission::Frame {
            name: translated,
            data,
            final_data,
        })
    }
}
