use super::context::Context;
use cma_sdk::{Result, responses_v2::ItemOptions};

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let api = c.client.responses_v2();
    match c.case.operation.as_str() {
        "GET /api/v2/agents/{agent_id}/responses/{response_id}/controls" => c.check(
            api.observe_controls(c.param("agent_id"), c.param("response_id"))
                .await,
        ),
        "POST /api/v2/agents/{agent_id}/responses/{response_id}/controls" => {
            let key = c
                .case
                .request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("idempotency-key"))
                .map(|(_, value)| value.as_str())
                .unwrap_or_default();
            c.check(
                api.submit_control(c.param("agent_id"), c.param("response_id"), &c.body()?, key)
                    .await,
            )
        }
        "POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt" => {
            let key = c
                .case
                .request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("idempotency-key"))
                .map(|(_, value)| value.as_str())
                .unwrap_or_default();
            c.check(
                api.interrupt_with_input(
                    c.param("agent_id"),
                    c.param("response_id"),
                    &c.body()?,
                    key,
                )
                .await,
            )
        }
        "GET /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation" => c.check(
            api.observe_queued_cancellation(c.param("agent_id"), c.param("response_id"))
                .await,
        ),
        "POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation" => {
            let key = c
                .case
                .request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("idempotency-key"))
                .map(|(_, value)| value.as_str())
                .unwrap_or_default();
            c.check(
                api.cancel_queued_response(
                    c.param("agent_id"),
                    c.param("response_id"),
                    &c.body()?,
                    key,
                )
                .await,
            )
        }
        "GET /api/v2/agents/{agent_id}/queued-inputs" => {
            c.check(api.list_queued_inputs(c.param("agent_id")).await)
        }
        "POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-input" => {
            let key = c
                .case
                .request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("idempotency-key"))
                .map(|(_, value)| value.as_str())
                .unwrap_or_default();
            c.check(
                api.edit_queued_input(c.param("agent_id"), c.param("response_id"), &c.body()?, key)
                    .await,
            )
        }
        "GET /api/v2/commands/{command_id}" => {
            c.check(api.read_command(c.param("command_id")).await)
        }
        "GET /api/v2" => c.check(api.read_capabilities().await),
        "GET /api/v2/conversations/{conversation_id}" => {
            c.check(api.read_conversation(c.param("conversation_id")).await)
        }
        "GET /api/v2/conversations/{conversation_id}/items" => c.check(
            api.list_items(
                c.param("conversation_id"),
                &ItemOptions {
                    after: c.q("after"),
                    before: c.q("before"),
                    order: c.tag("order")?,
                    limit: c.number("limit")?,
                },
            )
            .await,
        ),
        "GET /api/v2/responses/{response_id}" => c.check(api.read(c.param("response_id")).await),
        operation => {
            let key = c
                .case
                .request
                .headers
                .get("idempotency-key")
                .map(String::as_str);
            let mut stream = match operation {
                "POST /api/v2/responses" => api.create(&c.body()?, key).await?,
                "POST /api/v2/agents/{agent_id}/responses" => {
                    api.create_for_agent(c.param("agent_id"), &c.body()?, key)
                        .await?
                }
                "GET /api/v2/responses/{response_id}/events" => {
                    api.replay(c.param("response_id"), c.number("starting_after")?)
                        .await?
                }
                _ => panic!("unmapped v2 operation {operation}"),
            };
            assert_eq!(
                Some(&stream.receipt().response_id),
                c.case.response.headers.get("x-cma-response-id")
            );
            assert_eq!(
                Some(&stream.receipt().command_id),
                c.case.response.headers.get("x-cma-command-id")
            );
            for frame in c
                .case
                .response
                .body
                .as_array()
                .expect("Responses v2 SSE fixture")
            {
                let actual = stream.next().await.expect("expected v2 event")?;
                let expected = serde_json::from_value(frame["data"].clone()).unwrap();
                assert_eq!(actual, expected, "{} Responses v2 event", c.case.id);
            }
            assert!(stream.next().await.is_none());
            Ok(())
        }
    }
}
