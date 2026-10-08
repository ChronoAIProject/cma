//! A call-time snapshot cannot replace a running turn's admission authority.
use super::{
    default_fixtures::{
        completed, custom_event, default_channel, recording, run, telegram_default,
        telegram_messages, tool, CHAT,
    },
    fake_http::FakeHttp,
    group_fixtures::reviewer,
};
use cmaeg::{delivery::intents::DeliveryOutcome, turns::outcomes::TurnOutcome};
use cmaeg_core::seal::ReplyAddress;
use serde_json::json;

#[tokio::test(start_paused = true)]
async fn a_newer_tool_snapshot_cannot_redirect_another_consumers_running_turn() {
    for tool_first in [false, true] {
        for refused in [false, true] {
            let before = default_channel(telegram_default(), |_| {});
            let after = default_channel(telegram_default(), |value| {
                value["routing"]["default_reply"]["chat_id"] = "-1009".into();
                value["routing"]["default_consumer"]["reply"] = json!({"auto_final":false});
            });
            let mut recording = recording(FakeHttp::platforms(), true);
            let http = recording.http.clone();
            let lead = custom_event(&before, None, ReplyAddress::None);
            if !tool_first {
                run(
                    &recording,
                    &before,
                    &lead,
                    [TurnOutcome::Accepted {
                        response_id: "old-turn".into(),
                    }],
                )
                .await;
            }
            let review = custom_event(&after, Some(reviewer()), ReplyAddress::None);
            assert_eq!(lead.event_id, review.event_id);
            let message = if refused {
                json!({"blocks":[{"type":"buttons","id":"pick","options":[{"id":"a","label":"A"}]}]})
            } else {
                json!({"text":"New tool."})
            };
            let result = tool(&mut recording, &after, &review, message).await;
            if refused {
                assert!(matches!(result.delivery, DeliveryOutcome::Rejected { .. }));
            } else {
                assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            }
            run(&recording, &before, &lead, [completed("Old turn.")]).await;
            recording.finish().await;
            let chats = telegram_messages(&http.sent());
            assert_eq!(
                chats.last(),
                Some(&(CHAT.into(), Some("7".into()), "[lead] Old turn.".into()))
            );
            assert_eq!(chats.len(), if refused { 1 } else { 2 });
            if !refused {
                assert_eq!(
                    chats[0],
                    (
                        "-1009".into(),
                        Some("7".into()),
                        "[reviewer] New tool.".into()
                    )
                );
            }
        }
    }
}
