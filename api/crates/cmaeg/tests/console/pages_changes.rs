//! Record-keyed source changes for the page tests: the console's
//! `SourceChanges` seam over the fake CMA.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use cmaeg::{
    auth::Creator,
    channels::SourceRequest,
    console::{
        memory::ConsoleMemory,
        observer::Observation,
        source_changes::{
            AttachRequest, Attached, SourceAvailability, SourceChangeError, SourceChanges,
        },
    },
};
use cmaeg_core::{
    channel::{ChannelDefinition, Source, SourceId},
    ids::RecordId,
};
use futures_util::future::BoxFuture;

use super::pages_support::FakeReader;

/// Record-keyed source changes over the fake CMA, as #999's management
/// functions make them: owner check, compare-and-swap on the version, then
/// the index observer's report of the new version. Every call is counted
/// and every HMAC secret it was given is kept for the leak checks.
pub struct RecordingSources {
    pub availability: Mutex<SourceAvailability>,
    reader: Arc<FakeReader>,
    memory: Arc<ConsoleMemory>,
    pub calls: AtomicUsize,
    /// `(record, source id, expected version)` of every completed detach.
    pub detached: Mutex<Vec<(RecordId, SourceId, u64)>>,
    pub secrets: Mutex<Vec<String>>,
    /// A source id detach refuses as the default reply source.
    pub default_reply: Mutex<Option<String>>,
}

impl RecordingSources {
    pub fn new(
        availability: SourceAvailability,
        reader: Arc<FakeReader>,
        memory: Arc<ConsoleMemory>,
    ) -> Self {
        Self {
            availability: Mutex::new(availability),
            reader,
            memory,
            calls: AtomicUsize::new(0),
            detached: Mutex::default(),
            secrets: Mutex::default(),
            default_reply: Mutex::default(),
        }
    }

    /// The current definition and version of `record` for `creator`, or
    /// the refusal management would make.
    fn current(
        &self,
        creator: &Creator,
        record: &RecordId,
        expected: u64,
    ) -> Result<(ChannelDefinition, u64), SourceChangeError> {
        let channels = self.reader.channels.lock().unwrap();
        let (definition, version) = channels
            .get(record)
            .filter(|(definition, _)| definition.owner.subject == creator.subject)
            .ok_or(SourceChangeError::NotFound)?;
        if *version != expected {
            return Err(SourceChangeError::Conflict);
        }
        Ok((definition.clone(), *version))
    }

    fn write(&self, definition: ChannelDefinition, version: u64) {
        let record = definition.record_id.clone();
        let owner = definition.owner.subject.clone();
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
}

impl SourceChanges for RecordingSources {
    fn availability(&self) -> SourceAvailability {
        *self.availability.lock().unwrap()
    }

    fn attach<'a>(
        &'a self,
        creator: &'a Creator,
        record: &'a RecordId,
        request: AttachRequest,
    ) -> BoxFuture<'a, Result<Attached, SourceChangeError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (mut definition, version) =
                self.current(creator, record, request.expected_version)?;
            self.secrets.lock().unwrap().extend(
                request
                    .credentials
                    .hmac_secrets
                    .iter()
                    .map(|secret| secret.secret.as_str().to_owned()),
            );
            let source = match request.source {
                SourceRequest::NativeLark(source) => Source::NativeLark(source),
                SourceRequest::Whatsapp {
                    message_events,
                    app_id,
                    waba_id,
                    phone_number_id,
                    group_ids,
                    forms_enabled,
                    admission,
                } => Source::Whatsapp {
                    message_events,
                    app_id,
                    waba_id,
                    phone_number_id,
                    group_ids,
                    forms_enabled,
                    admission,
                },
                SourceRequest::Telegram { bot_id, .. } if bot_id == "refuse" => {
                    return Err(SourceChangeError::Invalid {
                        pointer: "/source/bot_id".into(),
                        message: "must be a Telegram bot id".into(),
                    })
                }
                SourceRequest::Telegram {
                    topic_lifecycle,
                    topic_creation,
                    message_events,
                    channel_posts,
                    member_join_chat_ids,
                    bot_id,
                    bot_username,
                    admission,
                } => Source::Telegram {
                    topic_lifecycle,
                    topic_creation,
                    message_events,
                    channel_posts,
                    member_join_chat_ids: member_join_chat_ids.unwrap_or_default(),
                    bot_id,
                    bot_username,
                    admission,
                },
                SourceRequest::Discord {
                    bot_id,
                    intents,
                    thread_history,
                    thread_lifecycle,
                    conversation_history,
                    admission,
                } => Source::Discord {
                    bot_id,
                    intents,
                    thread_history,
                    thread_lifecycle,
                    conversation_history,
                    admission,
                },
                SourceRequest::NyxidRelay {
                    issuer,
                    key_id,
                    route_ids,
                    platform,
                    bot_username,
                    bot_id,
                    trust_normalized,
                    verify_raw_event,
                    lark_message_effects,
                    lark_resources,
                    member_join_chat_ids,
                    admission,
                } => Source::NyxidRelay {
                    jwks_url: format!("{issuer}/jwks"),
                    issuer,
                    key_id,
                    route_ids,
                    platform,
                    bot_username,
                    bot_id,
                    trust_normalized,
                    verify_raw_event,
                    lark_message_effects,
                    lark_resources,
                    member_join_chat_ids: member_join_chat_ids.unwrap_or_default(),
                    admission,
                },
                SourceRequest::Custom {
                    auth,
                    prompt_template,
                    schema,
                    attachments,
                } => Source::Custom {
                    auth,
                    prompt_template,
                    schema,
                    attachments,
                },
                SourceRequest::Github {
                    app_ref,
                    installation_ids,
                    repository_ids,
                    deny_repository_ids,
                    events,
                    actions,
                    prompt_template,
                } => Source::Github {
                    app_ref,
                    installation_ids,
                    repository_ids,
                    deny_repository_ids,
                    events,
                    actions,
                    prompt_template,
                },
            };
            let id = source.source_id();
            definition.sources.push(source);
            self.write(definition, version + 1);
            Ok(Attached {
                source_id: id,
                version: version + 1,
            })
        })
    }

    fn detach<'a>(
        &'a self,
        creator: &'a Creator,
        record: &'a RecordId,
        source_id: &'a SourceId,
        expected_version: u64,
    ) -> BoxFuture<'a, Result<u64, SourceChangeError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (mut definition, version) = self.current(creator, record, expected_version)?;
            if self.default_reply.lock().unwrap().as_deref() == Some(source_id.as_str()) {
                return Err(SourceChangeError::DefaultReplyInUse);
            }
            let before = definition.sources.len();
            definition
                .sources
                .retain(|source| source.source_id() != *source_id);
            if definition.sources.len() == before {
                return Err(SourceChangeError::NotFound);
            }
            self.write(definition, version + 1);
            self.detached.lock().unwrap().push((
                record.clone(),
                source_id.clone(),
                expected_version,
            ));
            Ok(version + 1)
        })
    }
}
