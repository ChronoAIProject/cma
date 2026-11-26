use super::{
    cursor::{self, DetailCursor, PageCursor},
    model::*,
    payload::{self, Root},
    repository::*,
    ActivityHistory, Error,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use time::format_description::well_known::Rfc3339;

const MAX_RANGE_MS: i64 = 7 * 86_400_000;
const SESSION_MS: i64 = 15 * 60_000;
const PAGE_BYTES: usize = 1_048_576;
pub(super) fn timestamp(raw: &str) -> Result<i64, Error> {
    let at = time::OffsetDateTime::parse(raw, &Rfc3339).map_err(|_| Error::InvalidRequest)?;
    if !at.offset().is_utc() || at.nanosecond() % 1_000_000 != 0 {
        return Err(Error::InvalidRequest);
    }
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).map_err(|_| Error::InvalidRequest)
}
pub(super) fn format_time(ms: i64) -> Result<String, Error> {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .map_err(|_| Error::InvalidRequest)?
        .format(&Rfc3339)
        .map_err(|_| Error::InvalidRequest)
}
impl ActivityHistory {
    pub async fn page(
        &self,
        owner: &str,
        record: &str,
        query: HistoryQuery,
    ) -> Result<HistoryPage, Error> {
        self.page_with_clock(owner, record, query, super::now_ms)
            .await
    }

    pub(super) async fn page_with_clock(
        &self,
        owner: &str,
        record: &str,
        query: HistoryQuery,
        now_ms: impl Fn() -> i64,
    ) -> Result<HistoryPage, Error> {
        let repo = self.reader()?;
        let now = now_ms();
        let scope = super::scope(owner, record);
        let mut state = if let Some(token) = query.cursor.as_ref() {
            let cursor: PageCursor = cursor::open(&self.keys, cursor::HISTORY, token)?;
            if cursor.scope != scope
                || cursor.storage_generation != 1
                || cursor.retention_days != self.config.retention_days
                || query
                    .from
                    .as_deref()
                    .map(timestamp)
                    .transpose()?
                    .is_some_and(|v| v != cursor.from_ms)
                || query
                    .until
                    .as_deref()
                    .map(timestamp)
                    .transpose()?
                    .is_some_and(|v| v != cursor.until_ms)
                || query.limit.is_some_and(|v| v != cursor.limit)
            {
                return Err(Error::InvalidRequest);
            }
            cursor
        } else {
            let from_ms = timestamp(query.from.as_deref().ok_or(Error::InvalidRequest)?)?;
            let until_ms = timestamp(query.until.as_deref().ok_or(Error::InvalidRequest)?)?;
            let limit = query.limit.unwrap_or(100);
            if from_ms >= until_ms
                || until_ms
                    .checked_sub(from_ms)
                    .is_none_or(|v| v > MAX_RANGE_MS)
                || !(1..=200).contains(&limit)
                || until_ms > now
            {
                return Err(Error::InvalidRequest);
            }
            let snapshot = repo
                .snapshot(Scan {
                    scope: scope.clone(),
                    from_ms,
                    until_ms,
                    watermark: 0,
                    after: None,
                    limit: 1,
                    now_ms: now,
                })
                .await?;
            PageCursor {
                scope: scope.clone(),
                storage_generation: 1,
                from_ms,
                until_ms,
                limit,
                watermark: snapshot.watermark,
                after: None,
                deadline_ms: (now + SESSION_MS)
                    .min(snapshot.earliest_expiry_ms.unwrap_or(i64::MAX)),
                coverage_revision: snapshot.boundary.revision,
                unreadable: false,
                legacy_partial: false,
                retention_days: self.config.retention_days,
            }
        };
        if state.deadline_ms <= now {
            return Err(Error::Expired);
        }
        let rows = repo
            .scan(Scan {
                scope: scope.clone(),
                from_ms: state.from_ms,
                until_ms: state.until_ms,
                watermark: state.watermark,
                after: state.after.clone(),
                limit: usize::from(state.limit) + 1,
                now_ms: now,
            })
            .await?;
        let mut items = Vec::new();
        let mut size = 16_384; // Reserve bounded cursor/range/coverage envelope space.
        let mut has_more = false;
        for (scanned, stored) in rows.into_iter().enumerate() {
            if scanned == usize::from(state.limit) {
                has_more = true;
                break;
            }
            match Root::decode(&self.keys, &stored) {
                Ok(root) => {
                    let detail_cursor = root
                        .manifest
                        .clone()
                        .map(|manifest| {
                            cursor::seal(
                                &self.keys,
                                cursor::DETAILS,
                                &DetailCursor {
                                    scope: scope.clone(),
                                    event_key: stored.key.event_key.clone(),
                                    manifest,
                                    next: 0,
                                    deadline_ms: state.deadline_ms,
                                    expires_ms: root.expires_ms,
                                    incomplete: root.detail_incomplete,
                                    truncated: root.detail_truncated,
                                },
                            )
                        })
                        .transpose()?;
                    let item = HistoryItem {
                        history_event_id: stored.key.event_key.clone(),
                        activity: root.inline.unwrap_or(root.receipt),
                        detail_cursor,
                        detail_incomplete: root.detail_incomplete,
                        detail_truncated: root.detail_truncated,
                        legacy_partial: root.legacy_partial,
                    };
                    let bytes = serde_json::to_vec(&item)
                        .map_err(|_| Error::Unreadable)?
                        .len();
                    if size + bytes > PAGE_BYTES {
                        has_more = true;
                        break;
                    }
                    size += bytes;
                    state.legacy_partial |= item.legacy_partial;
                    items.push(item);
                }
                Err(_) => {
                    state.unreadable = true;
                    metrics::counter!("cmaeg_activity_history_unreadable_total").increment(1);
                    tracing::warn!("activity history export encountered an unreadable receipt");
                }
            }
            state.after = Some(stored.key);
        }
        // Use the boundary and retention cutoff at completion. A receipt at
        // the start of the range can expire while the snapshot is being read
        // and disappear before the earliest-expiry lookup sees it.
        let boundary = repo.boundary().await?;
        let completed_ms = now_ms();
        if boundary.revision != state.coverage_revision || completed_ms >= state.deadline_ms {
            return Err(Error::Expired);
        }
        // The attested policy limits completeness even if a reader restarts with
        // a longer configured horizon. Older rows retain their original expiry.
        let retained_from = completed_ms
            - i64::from(self.config.retention_days.min(boundary.retention_days)) * 86_400_000
            + 1;
        let mut gaps = Vec::new();
        if state.from_ms < retained_from {
            gaps.push(CoverageGap::RetentionExpired);
        }
        if boundary.since_ms.is_none_or(|since| state.from_ms < since) || state.legacy_partial {
            gaps.push(CoverageGap::LegacyPartial);
        }
        if boundary
            .until_ms
            .is_some_and(|until| state.until_ms > until)
            || (!self.config.capture && boundary.until_ms.is_none())
            || self.config.retention_days != boundary.retention_days
        {
            gaps.push(CoverageGap::CaptureGap);
        }
        if state.unreadable {
            gaps.push(CoverageGap::UnreadableRecord);
        }
        let next_cursor = if has_more {
            Some(cursor::seal(&self.keys, cursor::HISTORY, &state)?)
        } else {
            None
        };
        Ok(HistoryPage {
            items,
            next_cursor,
            has_more,
            range: HistoryRange {
                from: format_time(state.from_ms)?,
                until: format_time(state.until_ms)?,
            },
            coverage: HistoryCoverage {
                complete_event_enumeration: gaps.is_empty(),
                gaps,
                retained_from: format_time(retained_from)?,
                all_writers_capture_from: boundary.since_ms.map(format_time).transpose()?,
                all_writers_capture_until: boundary.until_ms.map(format_time).transpose()?,
                current_summaries: true,
            },
        })
    }
    pub async fn details(
        &self,
        owner: &str,
        record: &str,
        event_key: &str,
        token: &str,
    ) -> Result<HistoryDetails, Error> {
        let repo = self.reader()?;
        let mut state: DetailCursor = cursor::open(&self.keys, cursor::DETAILS, token)?;
        if state.scope != super::scope(owner, record)
            || state.event_key != event_key
            || state.next >= state.manifest.parts.len()
            || state.manifest.parts.len() > 32
        {
            return Err(Error::InvalidRequest);
        }
        if super::now_ms() >= state.deadline_ms.min(state.expires_ms) {
            return Err(Error::Expired);
        }
        let end = (state.next + 16).min(state.manifest.parts.len());
        let mut parts = Vec::new();
        for index in state.next..end {
            let part = &state.manifest.parts[index];
            let bytes = payload::read_part(
                repo,
                &self.keys,
                &state.scope,
                &state.event_key,
                state.expires_ms,
                part,
                super::now_ms(),
            )
            .await?;
            parts.push(HistoryDetailPart {
                index,
                sha256: part.hash.clone(),
                data: STANDARD.encode(bytes),
            });
        }
        if super::now_ms() >= state.deadline_ms.min(state.expires_ms) {
            return Err(Error::Expired);
        }
        state.next = end;
        Ok(HistoryDetails {
            generation: state.manifest.generation.clone(),
            encoding: "base64-json-fragment".into(),
            parts,
            next_cursor: if end < state.manifest.parts.len() {
                Some(cursor::seal(&self.keys, cursor::DETAILS, &state)?)
            } else {
                None
            },
            detail_incomplete: state.incomplete,
            detail_truncated: state.truncated,
        })
    }
}
