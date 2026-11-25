//! A console over real channel memory whose CMA reads are fakes, for the
//! page tests: record heads and one-record reads are counted, and every
//! fixture channel carries every kind of secret so a page that leaked one
//! would be caught.
use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use cmaeg::{
    auth::Creator,
    channels::management::{ManagementError, RecordedChannel},
    console::{
        channel_ids::LinkError,
        cma_budget::{Pacing, RecordHeads},
        memory::{ConsoleMemory, Schedule, Sources},
        observer::{ActivitySeed, CausationSummary, DeliveryClass, Observation, RefusalSeed},
        reader::ChannelReader,
        source_changes::SourceAvailability,
    },
    records::{RecordError, RecordHead},
    turns::observer::{DeliveryStep, Destination, LaunchClass},
    ConsoleMemoryConfig,
};
use cmaeg_core::{
    channel::{
        AdmissionPolicy, ChannelCreation, ChannelDefinition, CustomAttachments, CustomAuth,
        GroupAdmission, IdentityAdmission, Platform, Secret, Source,
    },
    ids::{ChannelId, ConsumerId, RecordId},
    profile::NyxIdIdentityKind,
};
use futures_util::future::BoxFuture;
use time::OffsetDateTime;

use super::{
    pages_changes::RecordingSources,
    support::{csrf_token, Composition, Harness},
};

/// Every secret a fixture channel holds; none may appear on any page.
pub const SECRETS: &[&str] = &[
    "fixture-channel-key-SECRET",
    "fixture-hmac-SECRET",
    "fixture-agent-key-SECRET",
    "7000000001:fixture-bot-token-SECRET",
    "fixture-bot-token-SECRET",
    "fixture-webhook-SECRET",
];
/// The fixture agent key hash; only `sha256:…<last 8>` may appear.
pub const AGENT_KEY_HASH: &str =
    "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0112345678";

/// The `index`-th fixture record id.
pub fn record(index: usize) -> RecordId {
    RecordId::new(format!("00000000-0000-4000-8000-{index:012}")).unwrap()
}

/// The channel id a fixture record displays: the fake reader opens exactly
/// this id to it.
pub fn channel_id(record: &RecordId) -> ChannelId {
    ChannelId::new(format!("cmaeg1.ch.fixture-{record}")).unwrap()
}

/// A channel at `record` owned by `owner`, named `name`, with a Telegram,
/// a Lark relay and a custom HMAC source, and every secret set.
pub fn definition(record: &RecordId, owner: &str, name: &str) -> ChannelDefinition {
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    definition.record_id = record.clone();
    definition.owner.subject = owner.to_owned();
    if owner.starts_with("org-") {
        definition.owner.kind = NyxIdIdentityKind::Organization;
    }
    definition.name = name.to_owned();
    definition.sources = vec![
        Source::Telegram {
            topic_lifecycle: false,
            topic_creation: false,
            message_events: false,
            channel_posts: false,
            member_join_chat_ids: Vec::new(),
            bot_id: "7000000001".into(),
            bot_username: Some("fixture_bot".into()),
            admission: AdmissionPolicy::All {},
        },
        Source::NyxidRelay {
            issuer: "https://nyxid.example".into(),
            jwks_url: "https://nyxid.example/.well-known/jwks.json".into(),
            key_id: "relay-key-1".into(),
            route_ids: vec!["route-a".into(), "route-b".into()],
            platform: Platform::Lark,
            bot_username: Some("lark_fixture_bot".into()),
            bot_id: Some("ou_fixturebot".into()),
            trust_normalized: false,
            verify_raw_event: false,
            lark_message_effects: None,
            lark_resources: None,
            member_join_chat_ids: Vec::new(),
            admission: AdmissionPolicy::Scoped {
                bot_messages: Default::default(),
                senders: IdentityAdmission::Allowlist(vec!["ou_sender_fixture_1".into()]),
                chats: IdentityAdmission::Open,
                groups: GroupAdmission::Mention,
            },
        },
        Source::Custom {
            auth: CustomAuth::Hmac {
                key_id: "hmac-key-1".into(),
            },
            prompt_template: Some("Event {{type}}".into()),
            schema: None,
            attachments: Some(CustomAttachments {
                pointer: "/attachments".into(),
                origins: vec!["https://files.example".into()],
            }),
        },
    ];
    definition.secrets.agent_key_hash = AGENT_KEY_HASH.into();
    definition.secrets.channel_key = Some(Secret::new(SECRETS[0]));
    definition.secrets.hmac_secrets = vec![("hmac-key-1".into(), Secret::new(SECRETS[1]))];
    definition.secrets.provider_credential = Some(Secret::new(SECRETS[2]));
    definition.secrets.telegram_bot_token = Some(Secret::new(SECRETS[3]));
    definition.secrets.telegram_webhook_secret = Some(Secret::new(SECRETS[5]));
    definition.creation = Some(ChannelCreation {
        channel_id: channel_id(record),
        fingerprint: "fixture".into(),
        ready: true,
    });
    definition
}

/// Every tab of a channel's detail page.
pub const TABS: &[&str] = &[
    "overview",
    "sources",
    "consumers",
    "routing",
    "activity",
    "producers",
];
/// The event key of the fixture traffic's first event.
pub const EVENT: &str = "0123456789abcdef";
/// The event key of the fixture traffic's published follow-up.
pub const FOLLOW_UP: &str = "fedcba9876543210";

/// CMA's record listing, counted.
#[derive(Default)]
pub struct FakeHeads {
    pub heads: Mutex<Vec<RecordHead>>,
    pub lists: AtomicUsize,
}

impl RecordHeads for FakeHeads {
    fn list<'a>(
        &'a self,
        owner: Option<&'a str>,
        after: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<RecordHead>, RecordError>> {
        Box::pin(async move {
            self.lists.fetch_add(1, Ordering::SeqCst);
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

/// One-record reads with management's owner check, counted.
#[derive(Default)]
pub struct FakeReader {
    pub channels: Mutex<HashMap<RecordId, (ChannelDefinition, u64)>>,
    pub reads: AtomicUsize,
}

impl ChannelReader for FakeReader {
    fn read<'a>(
        &'a self,
        creator: &'a Creator,
        record: &'a RecordId,
    ) -> BoxFuture<'a, Result<RecordedChannel, ManagementError>> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let channels = self.channels.lock().unwrap();
            let (definition, version) = channels.get(record).ok_or(ManagementError::NotFound)?;
            if definition.owner.subject != creator.subject {
                return Err(ManagementError::NotFound);
            }
            Ok(RecordedChannel {
                retirement_expected_version: None,
                definition: definition.clone(),
                version: *version,
                record_bytes: 12_288,
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

/// A served console, its memory and the fakes behind it.
pub struct World {
    pub harness: Harness,
    pub memory: Arc<ConsoleMemory>,
    pub heads: Arc<FakeHeads>,
    pub reader: Arc<FakeReader>,
    /// The source changes the console makes, when composed with them.
    pub sources: Option<Arc<RecordingSources>>,
}

impl World {
    /// A console composed as this release ships it: no source changes.
    pub async fn start() -> Self {
        Self::compose(None).await
    }

    /// A console whose source changes are `availability`.
    pub async fn with_sources(availability: SourceAvailability) -> Self {
        Self::compose(Some(availability)).await
    }

    /// The recorded source changes.
    pub fn changes(&self) -> &RecordingSources {
        self.sources
            .as_deref()
            .expect("composed with source changes")
    }

    /// The CSRF value of `action` on `record` for session `sid`.
    pub fn csrf(&self, sid: &str, action: &str, record: &RecordId) -> String {
        let key = self
            .harness
            .console
            .sessions()
            .find(sid)
            .unwrap()
            .meta()
            .csrf_key;
        csrf_token(&key, action, record.as_str())
    }

    async fn compose(availability: Option<SourceAvailability>) -> Self {
        let heads = Arc::new(FakeHeads::default());
        let reader = Arc::new(FakeReader::default());
        let memory = Arc::new(
            ConsoleMemory::new(
                &ConsoleMemoryConfig::default(),
                Sources {
                    heads: heads.clone(),
                    reader: reader.clone(),
                },
                Schedule {
                    settle: Duration::ZERO,
                    resync: Duration::from_secs(3_600),
                    pacing: Pacing {
                        base: Duration::from_millis(1),
                    },
                },
            )
            .with_non_human_providers(BTreeSet::from(["cma_codex".to_owned()])),
        );
        let sources = availability.map(|availability| {
            Arc::new(RecordingSources::new(
                availability,
                reader.clone(),
                memory.clone(),
            ))
        });
        let mut composition = Composition {
            directory: memory.clone(),
            ..Composition::default()
        };
        if let Some(sources) = &sources {
            composition.source_changes = sources.clone();
        }
        let harness = Harness::enabled_with(composition).await;
        Self {
            harness,
            memory,
            heads,
            reader,
            sources,
        }
    }

    /// Store `definition` at `version` in the fake CMA and the index.
    pub fn store(&self, definition: ChannelDefinition, version: u64) {
        let record = definition.record_id.clone();
        let owner = definition.owner.subject.clone();
        self.heads.lock_push(RecordHead {
            record_id: record.to_string(),
            owner: owner.clone(),
            version,
        });
        self.reader
            .channels
            .lock()
            .unwrap()
            .insert(record.clone(), (definition, version));
        self.memory.apply(Observation::IndexChanged {
            record,
            owner,
            version,
        });
    }

    /// A fixture channel at record `index`.
    pub fn add(&self, index: usize, owner: &str, name: &str) -> RecordId {
        let record = record(index);
        self.store(definition(&record, owner, name), 1);
        record
    }

    /// Traffic on `record`: a relayed Lark message accepted and answered by
    /// the default consumer, a follow-up it published, a refused event and
    /// a custom producer named `producer`.
    pub fn traffic(&self, record: &RecordId, producer: &str) {
        let default = ConsumerId::default_consumer();
        let seed = |key: &str| ActivitySeed {
            at: OffsetDateTime::now_utc(),
            event_key: key.into(),
            source: "relay_lark".into(),
            kind: "message",
            actor_masked: "…5678".into(),
            chat_masked: "…7890".into(),
            attachments: [1, 0, 0, 2],
            routing: None,
            trace: None,
            refusal: None,
            causation: None,
        };
        for observation in [
            Observation::Routed {
                record: record.clone(),
                entry_seed: Box::new(seed(EVENT)),
            },
            Observation::Launch {
                record: record.clone(),
                event_key: EVENT.into(),
                consumer: default.clone(),
                outcome: LaunchClass::Accepted,
                accepted_ms: Some(120),
            },
            Observation::Delivery {
                record: record.clone(),
                event_key: EVENT.into(),
                consumer: default.clone(),
                step: DeliveryStep::Answer,
                outcome: DeliveryClass::Confirmed,
                destination: Destination::Origin,
            },
            Observation::Routed {
                record: record.clone(),
                entry_seed: Box::new(seed(FOLLOW_UP)),
            },
            Observation::Published {
                record: record.clone(),
                event_key: FOLLOW_UP.into(),
                causation: CausationSummary {
                    parent_key: EVENT.into(),
                    hop: 1,
                    published_by: default.clone(),
                },
            },
            Observation::Delivery {
                record: record.clone(),
                event_key: FOLLOW_UP.into(),
                consumer: default.clone(),
                step: DeliveryStep::Failure,
                outcome: DeliveryClass::Rejected {
                    reason: "chat_not_found".into(),
                },
                destination: Destination::Default,
            },
            Observation::Refused {
                record: record.clone(),
                seed: RefusalSeed {
                    at: OffsetDateTime::now_utc(),
                    event_key: None,
                    source: "custom",
                    reason: "schema_mismatch".into(),
                },
            },
            Observation::Producer {
                record: record.clone(),
                source_uri: producer.into(),
                hint: "hmac:hmac-key-1".into(),
                ce_type: "com.example.order".into(),
                outcome: LaunchClass::Accepted,
            },
        ] {
            self.memory.apply(observation);
        }
    }

    pub fn reads(&self) -> usize {
        self.reader.reads.load(Ordering::SeqCst)
    }

    pub async fn close(self) {
        self.harness.close().await;
    }
}

impl FakeHeads {
    fn lock_push(&self, head: RecordHead) {
        let mut heads = self.heads.lock().unwrap();
        heads.retain(|known| known.record_id != head.record_id);
        heads.push(head);
        heads.sort_by(|a, b| a.record_id.cmp(&b.record_id));
    }
}
