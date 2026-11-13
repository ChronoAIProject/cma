//! Pure progress transitions; persistence supplies revision CAS.
use super::*;

impl LedgerCollectorProgressV1 {
    pub fn apply(&self, mutation: LedgerCollectorMutation) -> Result<Self> {
        use LedgerCollectorMutation::*;
        match mutation {
            Claim {
                holder_sha256,
                now,
                expires_at,
            } => self.claim(holder_sha256, now, expires_at),
            PinPage { fence, now, page } => self.pin_page(fence, now, page),
            Select {
                fence,
                now,
                pending,
            } => self.select(fence, now, pending),
            Outcome {
                fence,
                now,
                operation_id,
                phase,
                receipt_sha256,
            } => self.record_outcome(fence, now, &operation_id, phase, receipt_sha256),
            Settle {
                fence,
                now,
                operation_id,
            } => self.settle_group(fence, now, &operation_id),
            Advance {
                fence,
                now,
                active_revision,
                page_sha256,
            } => self.advance(fence, now, active_revision, &page_sha256),
            Rollover {
                fence,
                now,
                active_revision,
            } => self.rollover(fence, now, active_revision),
        }
    }

    pub fn new(
        binding_sha256: String,
        maintenance_epoch: u64,
        active_revision: u64,
        holder_sha256: String,
        now: i64,
        expires_at: i64,
    ) -> Result<Self> {
        let value = Self {
            format: FORMAT.into(),
            revision: 1,
            binding_sha256,
            maintenance_epoch,
            active_revision,
            sweep: 1,
            cursor: None,
            complete: false,
            lease: LedgerCollectionLease {
                holder_sha256,
                fence: 1,
                expires_at,
            },
            page: None,
            pending: None,
            last_ack_sha256: None,
        };
        if !value.validate() || now < 0 || expires_at <= now {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        Ok(value)
    }

    pub fn validate(&self) -> bool {
        self.format == FORMAT
            && self.revision > 0
            && digest(&self.binding_sha256)
            && self.maintenance_epoch > 0
            && self.active_revision > 0
            && self.sweep > 0
            && self
                .cursor
                .as_ref()
                .is_none_or(|v| !v.is_empty() && v.len() <= MAX_CURSOR)
            && digest(&self.lease.holder_sha256)
            && self.lease.fence > 0
            && self.lease.expires_at > 0
            && self.last_ack_sha256.as_ref().is_none_or(|s| digest(s))
            && (!self.complete || (self.pending.is_none() && self.page.is_none()))
            && self.page.as_ref().is_none_or(|p| {
                digest(&p.page_sha256)
                    && p.active_revision == self.active_revision
                    && p.cursor == self.cursor
                    && p.complete != p.next_cursor.is_some()
                    && (p.complete || p.cursor != p.next_cursor)
                    && p.next_cursor
                        .as_ref()
                        .is_none_or(|s| !s.is_empty() && s.len() <= MAX_CURSOR)
                    && p.remaining_groups.len() <= 64
                    && p.remaining_groups.iter().all(|s| digest(s))
                    && p.remaining_groups.windows(2).all(|pair| pair[0] < pair[1])
            })
            && self.pending.as_ref().is_none_or(|p| {
                self.page
                    .as_ref()
                    .is_some_and(|page| page.remaining_groups.contains(&p.group_key))
                    && uuid::Uuid::parse_str(&p.operation_id)
                        .is_ok_and(|id| !id.is_nil() && id.to_string() == p.operation_id)
                    && [
                        &p.group_key,
                        &p.request_sha256,
                        &p.observation_sha256,
                        &p.source_sha256,
                    ]
                    .into_iter()
                    .all(|s| digest(s))
                    && match p.phase {
                        LedgerCollectionPhase::Selected | LedgerCollectionPhase::Retained => {
                            p.receipt_sha256.is_none()
                        }
                        _ => p.receipt_sha256.as_ref().is_some_and(|s| digest(s)),
                    }
            })
    }

    /// This changes the worker claim only. Pending identity and evidence survive.
    pub fn claim(&self, holder_sha256: String, now: i64, expires_at: i64) -> Result<Self> {
        if !self.validate() || !digest(&holder_sha256) || now < 0 || expires_at <= now {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        if self.lease.expires_at > now {
            return Err(LedgerCollectionProgressError::StaleClaim);
        }
        let mut next = self.bump()?;
        next.lease = LedgerCollectionLease {
            holder_sha256,
            fence: self
                .lease
                .fence
                .checked_add(1)
                .ok_or(LedgerCollectionProgressError::Invalid)?,
            expires_at,
        };
        Ok(next)
    }

    pub fn pin_page(
        &self,
        fence: u64,
        now: i64,
        page: LedgerCollectionPageProgress,
    ) -> Result<Self> {
        self.require_claim(fence, now)?;
        if self.pending.is_some() || self.page.is_some() || self.complete {
            return Err(LedgerCollectionProgressError::PendingTransition);
        }
        if page.active_revision != self.active_revision || page.cursor != self.cursor {
            return Err(LedgerCollectionProgressError::StaleCursor);
        }
        let mut next = self.bump()?;
        next.page = Some(page);
        if !next.validate() {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        Ok(next)
    }

    pub fn select(&self, fence: u64, now: i64, pending: LedgerCollectionPending) -> Result<Self> {
        self.require_claim(fence, now)?;
        if self.pending.is_some() || self.complete {
            return Err(LedgerCollectionProgressError::PendingTransition);
        }
        if pending.phase != LedgerCollectionPhase::Selected || pending.receipt_sha256.is_some() {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        let mut next = self.bump()?;
        next.pending = Some(pending);
        if !next.validate() {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        Ok(next)
    }

    /// Caller must have verified A's original receipt against the pinned request.
    /// This progression never issues authority to perform a source transition.
    pub fn record_outcome(
        &self,
        fence: u64,
        now: i64,
        operation: &str,
        phase: LedgerCollectionPhase,
        receipt_sha256: Option<String>,
    ) -> Result<Self> {
        self.require_claim(fence, now)?;
        let current = self
            .pending
            .as_ref()
            .ok_or(LedgerCollectionProgressError::PendingTransition)?;
        if current.operation_id != operation {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        if current.phase == phase && current.receipt_sha256 == receipt_sha256 {
            return Ok(self.clone());
        }
        use LedgerCollectionPhase::*;
        if !matches!(
            (current.phase, phase),
            (Selected, Prepared | Retained)
                | (Prepared, SourceCommitted)
                | (SourceCommitted, Acknowledged)
        ) {
            return Err(LedgerCollectionProgressError::PendingTransition);
        }
        let mut next = self.bump()?;
        let pending = next.pending.as_mut().expect("checked pending");
        pending.phase = phase;
        pending.receipt_sha256 = receipt_sha256;
        if !next.validate() {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        Ok(next)
    }

    /// Settle one classified group while retaining the rest of the pinned page.
    pub fn settle_group(&self, fence: u64, now: i64, operation: &str) -> Result<Self> {
        self.require_claim(fence, now)?;
        let pending = self
            .pending
            .as_ref()
            .ok_or(LedgerCollectionProgressError::PendingTransition)?;
        if pending.operation_id != operation
            || !matches!(
                pending.phase,
                LedgerCollectionPhase::Acknowledged | LedgerCollectionPhase::Retained
            )
        {
            return Err(LedgerCollectionProgressError::PendingTransition);
        }
        let mut next = self.bump()?;
        let pending = next.pending.take().expect("checked pending");
        if pending.phase == LedgerCollectionPhase::Acknowledged {
            next.last_ack_sha256 = pending.receipt_sha256;
        }
        next.page
            .as_mut()
            .expect("validated page")
            .remaining_groups
            .retain(|key| key != &pending.group_key);
        Ok(next)
    }

    /// Empty candidates alone do not end a sweep: consume only the exact pinned
    /// inventory end marker after every group is durably classified/acknowledged.
    pub fn advance(
        &self,
        fence: u64,
        now: i64,
        observed_revision: u64,
        page_sha256: &str,
    ) -> Result<Self> {
        self.require_claim(fence, now)?;
        if observed_revision != self.active_revision {
            return Err(LedgerCollectionProgressError::StaleCursor);
        }
        let page = self
            .page
            .as_ref()
            .ok_or(LedgerCollectionProgressError::StaleCursor)?;
        if page.page_sha256 != page_sha256 {
            return Err(LedgerCollectionProgressError::StaleCursor);
        }
        if self.pending.is_some() || !page.remaining_groups.is_empty() {
            return Err(LedgerCollectionProgressError::PendingTransition);
        }
        let mut next = self.bump()?;
        let page = next.page.take().expect("checked page");
        next.cursor = page.next_cursor;
        next.complete = page.complete;
        if !next.validate() {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        Ok(next)
    }

    /// Explicit stale-snapshot rollover cannot discard an unsettled operation.
    /// The scheduler retains its separate fair group position before this call.
    pub fn rollover(&self, fence: u64, now: i64, active_revision: u64) -> Result<Self> {
        self.require_claim(fence, now)?;
        if self.pending.is_some() {
            return Err(LedgerCollectionProgressError::PendingTransition);
        }
        if active_revision < self.active_revision
            || active_revision == 0
            || (active_revision == self.active_revision && !self.complete)
        {
            return Err(LedgerCollectionProgressError::StaleCursor);
        }
        let mut next = self.bump()?;
        next.active_revision = active_revision;
        next.sweep = self
            .sweep
            .checked_add(1)
            .ok_or(LedgerCollectionProgressError::Invalid)?;
        next.cursor = None;
        next.complete = false;
        next.page = None;
        Ok(next)
    }

    fn require_claim(&self, fence: u64, now: i64) -> Result<()> {
        if !self.validate() {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        if fence != self.lease.fence || now < 0 || now >= self.lease.expires_at {
            return Err(LedgerCollectionProgressError::StaleClaim);
        }
        Ok(())
    }

    fn bump(&self) -> Result<Self> {
        let mut next = self.clone();
        next.revision = self
            .revision
            .checked_add(1)
            .ok_or(LedgerCollectionProgressError::Invalid)?;
        Ok(next)
    }
}

fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
