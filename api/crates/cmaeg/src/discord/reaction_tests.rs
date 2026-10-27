use super::{
    dispatch_fixture::Setup,
    journal_model::{Authority, Inbox},
    reactions, Checkpoint, Dispatch, GatewayError,
};
use crate::{
    channels::write_gate::{check, WriteFlags},
    connections::store::index_id,
    ingress::routing_store::AdmittedPlan,
};
use cmaeg_core::{
    activity::{ActivityKind, ActorKind},
    channel::Source,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn sync_source(f: &mut Setup) {
    f.fixture.replace(&f.channel);
    Arc::get_mut(&mut f.journal).unwrap().authority =
        Authority::from_definition(&f.channel).unwrap();
}
pub(crate) async fn setup() -> Setup {
    let mut f = Setup::new(true).await;
    let Source::Discord { intents, .. } = &mut f.channel.sources[0] else {
        panic!()
    };
    *intents |= super::REACTION_INTENTS;
    f.fixture.replace(&f.channel);
    let journal = Arc::get_mut(&mut f.journal).unwrap();
    journal.authority = Authority::from_definition(&f.channel).unwrap();
    journal.api.reactions = true;
    f.fixture
        .memory
        .reaction_users
        .lock()
        .unwrap()
        .insert("43".into(), json!({"id":"43","username":"reactor"}));
    f
}
pub(crate) fn event(f: &Setup, name: &str, sequence: u64) -> Dispatch {
    let mut raw = json!({"message_id":f.fixture.message_id(),"channel_id":"99","guild_id":"88"});
    if matches!(name, "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE") {
        raw["user_id"] = "43".into();
        raw["type"] = 0.into();
        raw["burst"] = false.into();
        if name == "MESSAGE_REACTION_ADD" {
            raw["member"] = json!({"user":{"id":"43","username":"reactor"}});
        }
    }
    if name != "MESSAGE_REACTION_REMOVE_ALL" {
        raw["emoji"] = json!({"id":null,"name":"👍"});
    }
    Dispatch {
        name: name.into(),
        sequence,
        data: raw,
    }
}
pub(crate) async fn persist(
    f: &Setup,
    event: &Dispatch,
    session: &str,
) -> Result<(), GatewayError> {
    f.journal
        .persist_reaction(
            &f.journal.authority,
            &Checkpoint {
                session_id: session.into(),
                resume_url: "wss://gateway.discord.gg/?v=10&encoding=json".into(),
                sequence: event.sequence,
            },
            event,
        )
        .await
}
async fn plans(f: &Setup) -> Vec<AdmittedPlan> {
    f.plans.recover_all_page(None, 32).await.unwrap().plans
}
#[tokio::test]
async fn discord_reactions_add_remove_add_and_clear_deltas_keep_original_conversation() {
    let mut f = setup().await;
    f.channel.partition.key = cmaeg_core::channel::PartitionKey::ConversationAndSender;
    f.fixture.replace(&f.channel);
    Arc::get_mut(&mut f.journal).unwrap().authority =
        Authority::from_definition(&f.channel).unwrap();
    f.message(|raw| raw["guild_id"] = "88".into()).await;
    f.sweep(f.now()).await;
    let original = plans(&f).await.remove(0);
    for (index, name) in [
        "MESSAGE_REACTION_ADD",
        "MESSAGE_REACTION_REMOVE",
        "MESSAGE_REACTION_ADD",
        "MESSAGE_REACTION_REMOVE_ALL",
        "MESSAGE_REACTION_REMOVE_EMOJI",
    ]
    .into_iter()
    .enumerate()
    {
        let change = event(&f, name, index as u64 + 2);
        persist(&f, &change, "gateway-a").await.unwrap();
        persist(&f, &change, "gateway-a").await.unwrap();
    }
    assert_eq!(f.sweep(f.now()).await.pending, 0);
    let plans = plans(&f).await;
    assert_eq!(plans.len(), 6);
    let mut operations = Vec::new();
    for plan in plans
        .iter()
        .filter(|plan| plan.identity != original.identity)
    {
        assert_eq!(
            plan.targets[0].launch.event.conversation_id,
            original.targets[0].launch.event.conversation_id
        );
        assert_eq!(
            plan.targets[0].launch.event.partition_key,
            original.targets[0].launch.event.partition_key
        );
        let ActivityKind::Event { ce, .. } = &plan.activity.kind else {
            panic!()
        };
        assert_eq!(ce.r#type, "cma.message.reaction.updated.v1");
        assert!(ce.time.is_none());
        let data = ce.data.as_ref().unwrap();
        assert_eq!(data["representation"], "delta");
        assert_eq!(data["target_author"]["id"], "42");
        assert!(data.get("old_reactions").is_none());
        assert!(data.get("new_reactions").is_none());
        let operation = data["operation"].as_str().unwrap();
        operations.push(operation.to_owned());
        if operation.starts_with("clear") {
            assert_eq!(plan.activity.actor.kind, ActorKind::Unknown);
            assert_eq!(plan.activity.actor.id, "unknown");
        } else {
            assert_eq!(plan.activity.actor.kind, ActorKind::Human);
            assert_eq!(plan.activity.actor.id, "43");
        }
    }
    operations.sort();
    assert_eq!(
        operations,
        ["add", "add", "clear_all", "clear_emoji", "remove"]
    );
    assert_eq!(f.direct_calls(), 0);
}
#[tokio::test]
async fn discord_reactions_session_identity_and_lost_custody_or_ack_are_immutable() {
    for committed in [false, true] {
        let f = setup().await;
        f.message(|raw| raw["guild_id"] = "88".into()).await;
        f.sweep(f.now()).await;
        let change = event(&f, "MESSAGE_REACTION_ADD", 2);
        let event_id = reactions::delivery_id(&f.journal.authority, "gateway-a", 2).unwrap();
        let id = index_id(
            "discord-native-message-event-v1",
            &[&f.fixture.record, "77", event_id.as_str()],
        );
        f.fixture.fail(&id, committed);
        assert_eq!(persist(&f, &change, "gateway-a").await.is_ok(), committed);
        persist(&f, &change, "gateway-a").await.unwrap();
        let first: Inbox = f.fixture.store.load(&id, true).await.unwrap().unwrap().0;
        f.fixture.advance(2000);
        persist(&f, &change, "gateway-a").await.unwrap();
        let restored: Inbox = f.fixture.store.load(&id, true).await.unwrap().unwrap().0;
        assert_eq!(first.payload_hash, restored.payload_hash);
        assert_eq!(first.received_at, restored.received_at);
        let mut conflict = Dispatch {
            name: change.name.clone(),
            sequence: change.sequence,
            data: change.data.clone(),
        };
        conflict.data["emoji"]["name"] = "👎".into();
        assert_eq!(
            persist(&f, &conflict, "gateway-a").await,
            Err(GatewayError::Protocol)
        );
        let store = super::inbox_scan::InboxStore::new(
            f.fixture.store.channels.clone(),
            f.fixture.store.keys.clone(),
        );
        let entry = store
            .page(None, f.now())
            .await
            .unwrap()
            .entries
            .into_iter()
            .find(|entry| entry.id == id)
            .unwrap();
        f.fixture.fail(&entry.ack_id(), false);
        assert_eq!(f.sweep(f.now()).await.pending, 1);
        assert_eq!(f.sweep(f.now()).await.pending, 0);
        persist(&f, &change, "gateway-b").await.unwrap();
        f.sweep(f.now()).await;
        assert_eq!(plans(&f).await.len(), 3);
        assert_eq!(f.direct_calls(), 0);
    }
}
#[tokio::test]
async fn discord_reactions_refuse_unknown_targets_changed_sources_and_unproven_actors() {
    let f = setup().await;
    f.message(|raw| raw["guild_id"] = "88".into()).await;
    f.sweep(f.now()).await;
    for defect in 0..9 {
        let mut change = event(&f, "MESSAGE_REACTION_ADD", defect + 2);
        match defect {
            0 => change.data["message_id"] = "700".into(),
            1 => change.data["channel_id"] = "100".into(),
            2 => change.data["guild_id"] = "89".into(),
            3 => change.data["member"]["user"]["bot"] = true.into(),
            4 => change.data["member"]["user"]["id"] = "44".into(),
            5 => change.data["member"]["user"]["system"] = true.into(),
            6 => change.data["message_author_id"] = "43".into(),
            7 => change.data["emoji"] = Value::Null,
            8 => change.data["burst"] = true.into(),
            _ => unreachable!(),
        }
        persist(&f, &change, "gateway-a").await.unwrap();
    }
    let mut rotated = f.channel.clone();
    rotated.secrets.discord_bot_token =
        Some(cmaeg_core::channel::Secret::new("rotated-test-credential"));
    let changed = Authority::from_definition(&rotated).unwrap();
    let change = event(&f, "MESSAGE_REACTION_ADD", 99);
    f.journal
        .persist_reaction(
            &changed,
            &Checkpoint {
                session_id: "new".into(),
                resume_url: "wss://gateway.discord.gg".into(),
                sequence: 99,
            },
            &change,
        )
        .await
        .unwrap();
    f.sweep(f.now()).await;
    assert_eq!(plans(&f).await.len(), 1);
}
#[tokio::test]
async fn discord_reactions_gate_preserves_default_intents_and_checks_matching_scope() {
    let f = setup().await;
    let base = super::journal_fixture::definition();
    let flags = WriteFlags {
        native_discord: true,
        multi_consumer: true,
        ..Default::default()
    };
    assert_eq!(super::MESSAGE_INTENTS, 37377);
    assert_eq!(
        check(None, &f.channel, flags).unwrap_err().message,
        "discord_reactions_disabled"
    );
    check(
        None,
        &f.channel,
        WriteFlags {
            discord_reactions: true,
            ..flags
        },
    )
    .unwrap();
    check(Some(&f.channel), &f.channel, flags).unwrap();
    check(Some(&f.channel), &base, flags).unwrap();
    for intents in [32768 | 4096 | 1024, 32768 | 512 | 8192, 37377 | (1 << 20)] {
        let mut definition = f.channel.clone();
        let Source::Discord { intents: value, .. } = &mut definition.sources[0] else {
            panic!()
        };
        *value = intents;
        assert!(definition.validate().is_err());
        assert!(super::GatewayClient::new(
            cmaeg_core::channel::Secret::new("fixture"),
            "77".into(),
            intents
        )
        .is_err());
    }
}

#[tokio::test]
async fn discord_reactions_checkpoint_waits_for_custody_and_issuance_gate_does_not_change_legacy_messages(
) {
    use super::NativeDiscordJournal;
    let fixture = super::journal_fixture::Fixture::new().await;
    let mut definition = super::journal_fixture::definition();
    let Source::Discord { intents, .. } = &mut definition.sources[0] else {
        panic!()
    };
    *intents |= super::REACTION_INTENTS;
    fixture.replace(&definition);
    let mut journal = fixture.acquire().await.unwrap();
    journal.authorize_connection(None).await.unwrap();
    let mut checkpoint = Checkpoint {
        session_id: "fenced-react".into(),
        resume_url: "wss://gateway.discord.gg/?v=10&encoding=json".into(),
        sequence: 0,
    };
    journal.admit(&checkpoint,&Dispatch {name:"READY".into(),sequence:0,data:json!({"session_id":"fenced-react","resume_gateway_url":"wss://gateway.discord.gg/","user":{"id":"77","bot":true}})}).await.unwrap();
    let mut raw = super::normalization_tests::raw();
    raw["id"] = fixture.message_id().into();
    raw["guild_id"] = "88".into();
    checkpoint.sequence = 1;
    journal
        .admit(
            &checkpoint,
            &Dispatch {
                name: "MESSAGE_CREATE".into(),
                sequence: 1,
                data: raw,
            },
        )
        .await
        .unwrap();
    let mut event = Dispatch {
        name: "MESSAGE_REACTION_REMOVE_ALL".into(),
        sequence: 2,
        data: json!({"message_id":fixture.message_id(),"channel_id":"99","guild_id":"88"}),
    };
    checkpoint.sequence = 2;
    journal.admit(&checkpoint, &event).await.unwrap();
    let key = |sequence| {
        index_id(
            "discord-native-message-event-v1",
            &[
                &fixture.record,
                "77",
                reactions::delivery_id(&journal.authority, "fenced-react", sequence)
                    .unwrap()
                    .as_str(),
            ],
        )
    };
    assert!(fixture
        .store
        .load::<Inbox>(&key(2), true)
        .await
        .unwrap()
        .is_none());
    Arc::get_mut(&mut journal).unwrap().api.reactions = true;
    event.sequence = 3;
    checkpoint.sequence = 3;
    let id = index_id(
        "discord-native-message-event-v1",
        &[
            &fixture.record,
            "77",
            reactions::delivery_id(&journal.authority, "fenced-react", 3)
                .unwrap()
                .as_str(),
        ],
    );
    fixture.fail(&id, false);
    assert_eq!(
        journal.admit(&checkpoint, &event).await,
        Err(GatewayError::Unavailable)
    );
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 2);
    journal.admit(&checkpoint, &event).await.unwrap();
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 3);
    let retained: Inbox = fixture.store.load(&id, true).await.unwrap().unwrap().0;
    fixture.fail(&journal.id, false);
    event.sequence = 4;
    checkpoint.sequence = 4;
    assert_eq!(
        journal.admit(&checkpoint, &event).await,
        Err(GatewayError::Unavailable)
    );
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 3);
    journal.admit(&checkpoint, &event).await.unwrap();
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 4);
    assert_eq!(
        fixture
            .store
            .load::<Inbox>(&id, true)
            .await
            .unwrap()
            .unwrap()
            .0
            .payload_hash,
        retained.payload_hash
    );
}

#[tokio::test]
async fn discord_unknown_reaction_target_retries_then_expires_without_provider_work() {
    let f = setup().await;
    let change = event(&f, "MESSAGE_REACTION_ADD", 2);
    persist(&f, &change, "gateway-a").await.unwrap();
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    assert!(plans(&f).await.is_empty());
    f.fixture.advance(60_000);
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    assert_eq!(f.sweep(f.now()).await.pending, 0);
    let values: Vec<Value> = f
        .fixture
        .memory
        .rows
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(id, row)| {
            cmaeg_core::seal::open_interaction(&f.fixture.store.keys, id, &row.blob)
                .ok()
                .and_then(|opened| serde_json::from_slice(&opened.payload).ok())
        })
        .collect();
    assert!(values
        .iter()
        .any(|value| value["disposition"]["reason"]
            == "discord_reaction_target_confirmation_timed_out"));
    assert!(plans(&f).await.is_empty());
    assert_eq!(f.direct_calls(), 0);
}
