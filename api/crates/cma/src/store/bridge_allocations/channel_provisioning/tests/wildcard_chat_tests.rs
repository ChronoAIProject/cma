//! A configured NyxID route is distinct from the signed actual platform chat.
//! This composes CMA acceptance and allocation; it does not run NyxID routing.
use super::*;
use layer2_core::bridge::{invoke::*, outbox::OutboxItem};
use layer2_core::sandbox::store::*;
use mongodb::bson::Document;
use serde_json::{json, Value};

#[path = "wildcard_chat_tests/backlog_tests.rs"]
mod backlog;
#[path = "wildcard_chat_tests/execution.rs"]
mod execution;
#[path = "wildcard_chat_tests/fixture.rs"]
mod fixture;
#[path = "wildcard_chat_tests/http.rs"]
mod http;

async fn count(s: &fixture::Setup, collection: &str) -> u64 {
    s.client
        .database(&s.database)
        .collection::<Document>(collection)
        .count_documents(doc! {})
        .await
        .unwrap()
}

async fn sorted_items(f: &ReplyFixture) -> Vec<Document> {
    let mut items = f.items().await;
    items.sort_by(|a, b| a.get_str("_id").unwrap().cmp(b.get_str("_id").unwrap()));
    items
}

fn source_command(f: &ReplyFixture) {
    let envelope = &f.command.receipt.envelope;
    assert_eq!(envelope.agent_id, f.allocation.placement.target().agent_id);
    assert_eq!(envelope.source.reference, f.inbox.id);
    assert_eq!(envelope.correlation_id, f.inbox.id);
    match &envelope.actor {
        AgentActor::Channel {
            bridge_id,
            platform,
            chat_id,
            sender_id,
            grant_id,
        } => {
            assert_eq!(bridge_id, f.inbox.pin.bridge_id.as_str());
            assert_eq!(platform, "telegram");
            assert_eq!(chat_id, &f.inbox.envelope.chat_id);
            assert_eq!(sender_id, "12345");
            assert_eq!(grant_id, f.grant.id.as_str());
        }
        _ => panic!("the actual accepted Channel source must own the command"),
    }
    assert_eq!(
        f.allocation.first_source,
        PartitionSourcePin::for_inbox(&f.inbox).unwrap()
    );
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn signed_wildcard_route_concurrent_chats_allocate_distinct_agents_and_recover_original_receipts(
) {
    let s = fixture::Setup::new().await;
    for collection in [
        "bridge_allocations",
        "sandboxes",
        "workspaces",
        "agents",
        "agent_commands",
        "bridge_outbox_items",
    ] {
        assert_eq!(
            count(&s, collection).await,
            0,
            "no pre-created {collection}"
        );
    }
    let first = http::SignedChat::new("wildcard-chat-a", -100123);
    let second = http::SignedChat::new("wildcard-chat-b", -100456);
    let router = http::router(&s).await;
    let ((first_receipt, first_inbox), (second_receipt, second_inbox)) = tokio::join!(
        http::accept(router.clone(), &s, &first),
        http::accept(router, &s, &second),
    );
    assert_ne!(first_inbox.id, second_inbox.id);
    assert_eq!(first_inbox.pin, second_inbox.pin);
    assert_eq!(first_inbox.bot_id, second_inbox.bot_id);
    assert_eq!(
        first_inbox.envelope.route_id,
        second_inbox.envelope.route_id
    );
    assert_eq!(
        first_inbox.envelope.sender_id,
        second_inbox.envelope.sender_id
    );
    assert_ne!(first_inbox.envelope.chat_id, second_inbox.envelope.chat_id);
    assert_eq!(count(&s, "bridge_callback_inbox").await, 2);
    assert_eq!(count(&s, "bridge_allocations").await, 0);
    assert_eq!(count(&s, "agents").await, 0);

    let (a, b) = tokio::join!(
        s.reserve_request(&first_inbox),
        s.reserve_request(&second_inbox)
    );
    let (a, b) = tokio::join!(s.allocations.reserve(a), s.allocations.reserve(b));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.identity.key, b.identity.key);
    assert_ne!(a.id, b.id);
    assert_ne!(
        a.placement.target().sandbox_id,
        b.placement.target().sandbox_id
    );
    assert_ne!(
        a.placement.target().workspace_id,
        b.placement.target().workspace_id
    );
    assert_ne!(a.placement.target().agent_id, b.placement.target().agent_id);
    assert_eq!(
        a.first_source,
        PartitionSourcePin::for_inbox(&first_inbox).unwrap()
    );
    assert_eq!(
        b.first_source,
        PartitionSourcePin::for_inbox(&second_inbox).unwrap()
    );
    assert_eq!(count(&s, "bridge_allocations").await, 2);
    assert_eq!(
        count(&s, "agents").await,
        0,
        "reservation must not preseed target rows"
    );
    assert!(s
        .resources
        .get_agent(&a.placement.target().agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(s
        .resources
        .get_agent(&b.placement.target().agent_id)
        .await
        .unwrap()
        .is_none());

    // Acceptance and reservation raced above. Each provisioning run uses its
    // own local single-allocation issuer, with real driver/child/startup paths.
    let a = execution::start(s.partition(a).await, first_inbox.clone()).await;
    let b = execution::start(s.partition(b).await, second_inbox.clone()).await;
    source_command(&a);
    source_command(&b);
    assert_ne!(
        a.command.receipt.envelope.agent_id,
        b.command.receipt.envelope.agent_id
    );
    assert_ne!(
        a.command.receipt.envelope.command_id,
        b.command.receipt.envelope.command_id
    );
    assert_ne!(a.grant.id, b.grant.id);
    assert!(
        a.credential.expose() != b.credential.expose(),
        "distinct per-Agent tool credentials"
    );
    assert_eq!(count(&s, "agents").await, 2);
    assert_eq!(count(&s, "agent_commands").await, 2);
    assert!(a.items().await.is_empty());

    let (a_context, b_context) = tokio::join!(a.context(), b.context());
    assert_ne!(a_context, b_context);
    let (wrong_a, wrong_b) = tokio::join!(
        a.reply(&b_context, "wrong-chat-a", "Must not cross into B."),
        b.reply(&a_context, "wrong-chat-b", "Must not cross into A."),
    );
    assert_eq!(wrong_a["isError"], true, "{wrong_a}");
    assert_eq!(wrong_b["isError"], true, "{wrong_b}");
    assert!(a.items().await.is_empty());
    let (reply_a, reply_b) = tokio::join!(
        a.reply(&a_context, "local-chat-reply", "Progress for A."),
        b.reply(&b_context, "local-chat-reply", "Progress for B."),
    );
    assert_eq!(reply_a["isError"], false, "{reply_a}");
    assert_eq!(reply_b["isError"], false, "{reply_b}");
    assert_ne!(
        reply_a["structuredContent"]["receipt_id"],
        reply_b["structuredContent"]["receipt_id"]
    );
    let items = sorted_items(&a).await;
    assert_eq!(items.len(), 2);
    let mut chat_item_counts = [0, 0];
    for row in &items {
        let item: OutboxItem = crate::store::sandbox_mongo::read_record(row).unwrap();
        let own = if item.source.chat_id() == first.chat {
            chat_item_counts[0] += 1;
            &a
        } else {
            chat_item_counts[1] += 1;
            &b
        };
        assert_eq!(item.source.chat_id(), own.inbox.envelope.chat_id);
        assert_eq!(item.source.message_id(), own.inbox.message_id);
        assert_eq!(item.source.inbox_id(), own.inbox.id);
        assert_eq!(item.source.target(), own.allocation.placement.target());
        assert_eq!(
            item.source.command_id(),
            &own.command.receipt.envelope.command_id
        );
        assert_eq!(item.source.allocation_id(), own.allocation.id);
        assert_eq!(item.source.config().connector.route_ids, ["route-1"]);
        assert_eq!(item.source.config().connector.bot_id, "existing-bot");
        assert_eq!(item.source.config().connector.issuer, "https://nyx.example");
    }
    assert_eq!(
        chat_item_counts,
        [1, 1],
        "exactly one outgoing item per chat"
    );

    let reopened_router = http::router(&s).await;
    let ((recovered_a, recovered_a_inbox), (recovered_b, recovered_b_inbox)) = tokio::join!(
        http::accept(reopened_router.clone(), &s, &first),
        http::accept(reopened_router, &s, &second),
    );
    assert_eq!(recovered_a, first_receipt);
    assert_eq!(recovered_b, second_receipt);
    assert!(recovered_a_inbox == first_inbox);
    assert!(recovered_b_inbox == second_inbox);
    let reopened_bridges = Arc::new(open_store(&s.client, &s.database).await);
    let reopened_allocations =
        MongoBridgeAllocationStore::open(reopened_bridges, None, &s.database)
            .await
            .unwrap();
    let (again_a, again_b) = tokio::join!(
        s.reserve_request(&recovered_a_inbox),
        s.reserve_request(&recovered_b_inbox)
    );
    let (again_a, again_b) = tokio::join!(
        reopened_allocations.reserve(again_a),
        reopened_allocations.reserve(again_b)
    );
    assert!(again_a.unwrap() == a.allocation);
    assert!(again_b.unwrap() == b.allocation);
    assert_eq!(count(&s, "bridge_allocations").await, 2);
    assert_eq!(count(&s, "agents").await, 2);
    assert_eq!(count(&s, "agent_commands").await, 2);
    assert_eq!(sorted_items(&a).await, items);

    // Real assistant updates and turn completion remain CMA-only; only the two
    // explicit source-bound tool calls above created outgoing work.
    a.emit_assistant_text("chat-a-final", "Automatic output for A stays in CMA.")
        .await;
    b.emit_assistant_text("chat-b-final", "Automatic output for B stays in CMA.")
        .await;
    let ((), ()) = tokio::join!(a.finish(), b.finish());
    a.await_assistant_text("Automatic output for A stays in CMA.")
        .await;
    b.await_assistant_text("Automatic output for B stays in CMA.")
        .await;
    assert_eq!(sorted_items(&a).await, items);
    for (own, other_text) in [
        (&a, "Automatic output for B stays in CMA."),
        (&b, "Automatic output for A stays in CMA."),
    ] {
        let logs = own
            .resources
            .agent_log(&own.command.receipt.envelope.agent_id, 0, 100)
            .await
            .unwrap();
        assert!(!logs.iter().any(|log| matches!(&log.kind, layer2_core::sandbox::AgentLogKind::AgentMessage { content } if content == other_text)));
    }
    drop(a);
    drop(b);
    s.client.database(&s.database).drop().await.unwrap();
}
