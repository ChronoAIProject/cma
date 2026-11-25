//! Bounded period reads use caller-supplied authoritative window identities.
use super::*;

const MAX_WINDOWS: usize = 96;
const MAX_PERIOD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PeriodReadRequest {
    pub owner_id: String,
    pub sandbox_id: String,
    pub from_ms: u64,
    pub to_ms: u64,
    pub source_cutoff_ms: u64,
    /// Ordered exact window identities from the allocation/settlement plan.
    pub windows: Vec<EvidenceScope>,
}

impl MeasuredLedger {
    /// Read all named windows at one custody snapshot. A missing window or
    /// discontinuity refuses the period; no missing observation becomes zero.
    pub(crate) async fn read_period(
        &self,
        request: PeriodReadRequest,
    ) -> Result<Vec<FrozenEvidence>, CaptureError> {
        if request.windows.is_empty()
            || request.windows.len() > MAX_WINDOWS
            || request.from_ms >= request.to_ms
            || request.to_ms > request.source_cutoff_ms
        {
            return Err(CaptureError::Unavailable);
        }
        let mut next_start = request.from_ms;
        for scope in &request.windows {
            if scope.owner_id != request.owner_id
                || scope.sandbox_id != request.sandbox_id
                || scope.from_ms != next_start
                || scope.to_ms > request.to_ms
            {
                return Err(CaptureError::Unavailable);
            }
            scope.stream_id().map_err(|_| CaptureError::Corrupt)?;
            next_start = scope.to_ms;
        }
        if next_start != request.to_ms {
            return Err(CaptureError::Unavailable);
        }
        let mut session = self
            .database
            .client()
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let mut selected = Vec::with_capacity(request.windows.len());
        let mut bytes = 0;
        for scope in request.windows {
            let history = self
                .history(
                    &scope.stream_id().map_err(|_| CaptureError::Corrupt)?,
                    &scope,
                    &mut session,
                )
                .await?;
            let evidence = history
                .into_iter()
                .rev()
                .find(|batch| batch.source_cutoff_ms() <= request.source_cutoff_ms)
                .ok_or(CaptureError::Unavailable)?;
            bytes += serde_json::to_vec(&evidence)
                .map_err(|_| CaptureError::Corrupt)?
                .len();
            if bytes > MAX_PERIOD_BYTES {
                return Err(CaptureError::Unavailable);
            }
            selected.push(evidence);
        }
        if session.snapshot_time().is_none() {
            return Err(CaptureError::Unavailable);
        }
        Ok(selected)
    }
}
