//! Fixtures shared by the console's unit tests.
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
        SourceKind,
    },
    channel::{ChannelCreation, ChannelDefinition, Platform},
    ids::{ChannelId, EventId, RecordId},
};
use futures_util::future::BoxFuture;

use super::{
    authority::{Authorized, OwnerScope},
    channel_ids::LinkError,
    cma_budget::RecordHeads,
    reader::ChannelReader,
};
use crate::{
    auth::Creator,
    channels::management::{ManagementError, RecordedChannel},
    records::{RecordError, RecordHead},
};

/// A Personal authority for `subject`, as `ConsoleAuthority` derives it.
pub(crate) fn authorized(subject: &str) -> Authorized {
    Authorized::new(
        Creator {
            subject: subject.to_owned(),
            token_hash: "sha256:fixture".to_owned(),
            organization: false,
        },
        OwnerScope::Personal,
    )
}

/// The `index`-th fixture record id, a canonical UUID v4 that sorts by index.
pub(crate) fn record(index: usize) -> RecordId {
    RecordId::new(format!("00000000-0000-4000-8000-{index:012}")).unwrap()
}

/// CMA's record listing, recording when each request started and ended and
/// how many overlapped.
#[derive(Default)]
pub(crate) struct FakeHeads {
    pub heads: Mutex<Vec<RecordHead>>,
    pub calls: Mutex<Vec<(Option<String>, Instant, Instant)>>,
    pub inflight: AtomicUsize,
    pub max_inflight: AtomicUsize,
    pub delay: Duration,
    pub fail: Mutex<bool>,
}

impl FakeHeads {
    /// `count` records alternating between owners `alice` and `bob`.
    pub(crate) fn seeded(count: usize) -> Self {
        let heads = (0..count)
            .map(|index| RecordHead {
                record_id: record(index).to_string(),
                owner: if index % 2 == 0 { "alice" } else { "bob" }.to_owned(),
                version: 1,
            })
            .collect();
        Self {
            heads: Mutex::new(heads),
            delay: Duration::from_millis(5),
            ..Self::default()
        }
    }
}

impl RecordHeads for FakeHeads {
    fn list<'a>(
        &'a self,
        owner: Option<&'a str>,
        after: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<RecordHead>, RecordError>> {
        Box::pin(async move {
            let started = Instant::now();
            let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_inflight.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            self.calls
                .lock()
                .unwrap()
                .push((owner.map(str::to_owned), started, Instant::now()));
            if *self.fail.lock().unwrap() {
                return Err(RecordError::Unavailable("fixture outage".into()));
            }
            let heads = self.heads.lock().unwrap();
            Ok(heads
                .iter()
                .filter(|head| owner.is_none_or(|owner| head.owner == owner))
                .filter(|head| after.is_none_or(|after| head.record_id.as_str() > after))
                .take(limit)
                .cloned()
                .collect())
        })
    }
}

/// The fixture channel at `record`, owned by `subject`, named `name`, with a
/// retained creation channel id.
pub(crate) fn definition(record: &RecordId, subject: &str, name: &str) -> ChannelDefinition {
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    definition.record_id = record.clone();
    definition.owner.subject = subject.to_owned();
    definition.name = name.to_owned();
    definition.secrets.channel_key = Some(cmaeg_core::channel::Secret::new("fixture-channel-key"));
    definition.creation = Some(ChannelCreation {
        channel_id: channel_id(record),
        fingerprint: "fixture".into(),
        ready: true,
    });
    definition
}

/// A display-only channel id fixture for `record`.
pub(crate) fn channel_id(record: &RecordId) -> ChannelId {
    ChannelId::new(format!("cmaeg1.ch.fixture-{record}")).unwrap()
}

/// How a [`FakeReader`] read fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadFailure {
    Unreadable,
    Unavailable,
}

/// One-record reads with management's owner check, counting each read.
#[derive(Default)]
pub(crate) struct FakeReader {
    pub channels: Mutex<HashMap<RecordId, (ChannelDefinition, u64)>>,
    pub reads: AtomicUsize,
    pub failure: Mutex<Option<ReadFailure>>,
}

impl FakeReader {
    pub(crate) fn store(&self, definition: ChannelDefinition, version: u64) {
        self.channels
            .lock()
            .unwrap()
            .insert(definition.record_id.clone(), (definition, version));
    }
}

impl ChannelReader for FakeReader {
    fn read<'a>(
        &'a self,
        creator: &'a Creator,
        record: &'a RecordId,
    ) -> BoxFuture<'a, Result<RecordedChannel, ManagementError>> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match *self.failure.lock().unwrap() {
                Some(ReadFailure::Unreadable) => {
                    return Err(ManagementError::Internal("fixture unreadable".into()))
                }
                Some(ReadFailure::Unavailable) => {
                    return Err(ManagementError::Record(RecordError::Unavailable(
                        "fixture outage".into(),
                    )))
                }
                None => {}
            }
            let channels = self.channels.lock().unwrap();
            let (definition, version) = channels.get(record).ok_or(ManagementError::NotFound)?;
            if definition.owner.subject != creator.subject {
                return Err(ManagementError::NotFound);
            }
            Ok(RecordedChannel {
                definition: definition.clone(),
                version: *version,
                record_bytes: 2_048,
                retirement_expected_version: None,
                valid: true,
            })
        })
    }

    fn opens_to(&self, id: &ChannelId, record: &RecordId) -> Result<(), LinkError> {
        if !id.as_str().starts_with("cmaeg1.ch.fixture-") {
            return Err(LinkError::Malformed);
        }
        if *id != channel_id(record) {
            return Err(LinkError::Mismatch);
        }
        Ok(())
    }
}

/// A relayed Telegram message carrying text, an actor, a chat and a caption
/// that must never reach the console.
pub(crate) fn activity(event_id: &str) -> Activity {
    Activity {
        thread: None,
        schema_version: 1,
        event_id: EventId::new(event_id).unwrap(),
        kind: ActivityKind::Message {
            text: "fixture message text".into(),
            attachments: Vec::new(),
            mentions_bot: true,
            reply_to: None,
            quoted_message: None,
            album_id: None,
        },
        actor: Actor {
            id: "123456789".into(),
            display_name: Some("Fixture Person".into()),
            kind: ActorKind::Human,
        },
        conversation: ActivityConversation {
            id: "-1001234567890".into(),
            kind: ConversationKind::Group,
            thread_id: None,
        },
        occurred_at: "2026-10-08T00:00:00Z".into(),
        source: SourceKind::NyxidRelay {
            platform: Platform::Telegram,
            route_id: "route-1".into(),
        },
        routing: None,
    }
}
