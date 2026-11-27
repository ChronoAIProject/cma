use super::*;
use layer2_core::sandbox_usage_evidence::{freeze_measured, FrozenMeasuredMinute, MeasuredError};
use layer3_adapter::sandbox_metrics::PhysicalSeries;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub version: u8,
    pub retained_from: Option<String>,
    pub input: MeasuredInput,
    pub frozen: Option<FrozenMeasuredMinute>,
    pub hold: Option<String>,
    pub rejected_samples: Vec<PhysicalSeries>,
    pub rejected_sample_bits: Vec<Vec<[String; 2]>>,
}

pub(crate) struct MeasuredPublication {
    pub(super) manifest: Manifest,
    pub(super) bytes: Vec<u8>,
    pub(super) digest: String,
    pub(super) result_digest: Option<String>,
    pub(super) event_id: String,
    pub(super) opening_id: String,
    pub(super) fact_id: String,
}

impl MeasuredInput {
    pub(crate) fn freeze(
        self,
        series: Vec<PhysicalSeries>,
        complete: bool,
    ) -> Result<MeasuredPublication, SandboxStoreError> {
        let (frozen, rejected_samples, rejected_sample_bits, error) = match freeze_measured(
            self.expected.clone(),
            self.bindings(),
            series.clone(),
            complete,
        ) {
            Ok(value) => (Some(value), vec![], vec![], None),
            Err(MeasuredError::Attribution) => {
                let bits = series
                    .iter()
                    .map(|s| {
                        s.samples
                            .iter()
                            .map(|s| {
                                [
                                    format!("{:016x}", s.at.to_bits()),
                                    format!("{:016x}", s.value.to_bits()),
                                ]
                            })
                            .collect()
                    })
                    .collect();
                (
                    None,
                    series,
                    bits,
                    Some("physical_attribution_conflict".into()),
                )
            }
            Err(_) => return Err(unavailable("invalid measured input")),
        };
        let mut hold = self.hold.clone().or(error);
        if self
            .predecessor
            .as_ref()
            .is_some_and(|h| h.resource != self.resource)
        {
            hold = Some("immutable_resource_conflict".into());
        }
        if hold.is_none() {
            hold = self.refresh_hold(frozen.as_ref(), complete)?;
        }
        let result_digest = frozen.as_ref().map(|f| {
            hash(&serde_json::to_vec(&(f.fact(), &self.resource)).expect("finite integer fact"))
        });
        let fact_id = self
            .predecessor
            .as_ref()
            .map_or_else(|| uuid::Uuid::new_v4().to_string(), |h| h.fact_id.clone());
        let manifest = Manifest {
            version: 1,
            retained_from: None,
            input: self,
            frozen,
            hold,
            rejected_samples,
            rejected_sample_bits,
        };
        let bytes = serde_json::to_vec(&manifest).map_err(|_| corrupt())?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(unavailable("measured manifest bound"));
        }
        Ok(MeasuredPublication {
            digest: hash(&bytes),
            manifest,
            bytes,
            result_digest,
            event_id: uuid::Uuid::new_v4().to_string(),
            opening_id: uuid::Uuid::new_v4().to_string(),
            fact_id,
        })
    }
}
