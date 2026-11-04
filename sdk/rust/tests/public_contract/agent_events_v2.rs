use super::context::Context;
use cma_sdk::{
    Result,
    agent_events_v2::{AgentCursor, AgentFrame, AgentPageCursor, SnapshotOptions, TailOptions},
};

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let api = c.client.agent_events_v2();
    let tool_output = c
        .case
        .request
        .headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("prefer") && value == "cma-tool-output");
    match c.case.operation.as_str() {
        "GET /api/v2/ui/task-experience" => c.check(c.client.read_task_experience().await),
        "GET /api/v2/agents/{agent_id}/events/head" => c.check(api.head(c.param("agent_id")).await),
        "GET /api/v2/ui/transport" => c.check(api.read_transport_eligibility().await),
        "GET /api/v2/agents/{agent_id}/events/snapshot" => c.check(
            api.snapshot(
                c.param("agent_id"),
                &SnapshotOptions {
                    tool_output,
                    limit: c.number("limit")?,
                    page_cursor: c.q("page_cursor").map(AgentPageCursor::parse).transpose()?,
                },
            )
            .await,
        ),
        "GET /api/v2/agents/{agent_id}/events" => {
            let mut stream = api
                .events(
                    c.param("agent_id"),
                    &TailOptions {
                        tool_output,
                        after_cursor: c.q("after_cursor").map(AgentCursor::parse).transpose()?,
                        last_event_id: c
                            .case
                            .request
                            .headers
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case("last-event-id"))
                            .map(|(_, value)| AgentCursor::parse(value))
                            .transpose()?,
                        follow: c.number("follow")?,
                        live_timestamps: false,
                    },
                )
                .await?;
            for expected in c.case.response.body.as_array().expect("Agent SSE fixture") {
                let value = expected["data"].clone();
                let expected = match expected["event"].as_str().unwrap() {
                    "cma.agent.event" => AgentFrame::Event(serde_json::from_value(value).unwrap()),
                    "cma.agent.live" => AgentFrame::Live(serde_json::from_value(value).unwrap()),
                    "cma.agent.live_gap" => {
                        AgentFrame::LiveGap(serde_json::from_value(value).unwrap())
                    }
                    "cma.agent.error" => AgentFrame::Error(serde_json::from_value(value).unwrap()),
                    _ => panic!("unexpected Agent fixture event"),
                };
                stream
                    .consume_next(|actual| {
                        assert_eq!(actual, expected);
                        async { Ok(()) }
                    })
                    .await
                    .expect("expected Agent frame")?;
            }
            assert!(stream.consume_next(|_| async { Ok(()) }).await.is_none());
            Ok(())
        }
        _ => panic!("unmapped Agent event operation {}", c.case.operation),
    }
}
