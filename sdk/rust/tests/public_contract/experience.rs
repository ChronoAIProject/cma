use super::context::Context;
use cma_sdk::{
    Error, Result,
    agui::{AgentEvent, ReplayOptions},
    auth::{AuthSession, Identity},
    responses::ItemOptions,
};

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let client = c.client;
    match c.case.operation.as_str() {
        "GET /api/v1/agui/capabilities" => c.check(client.agui().read_capabilities().await),
        "GET /api/v1/agui/agents/{id}/sandbox" => {
            c.check(client.agui().read_sandbox(c.param("id")).await)
        }
        "GET /api/v1/agui/events" | "POST /api/v1/agui/run" => {
            let mut stream = if c.case.request.method == "POST" {
                let body = c.body()?;
                if let Some(key) = c.case.request.headers.get("idempotency-key") {
                    client.agui().run_with_key(&body, key).await?
                } else {
                    client.agui().run(&body).await?
                }
            } else {
                client
                    .agui()
                    .events(
                        &c.q("thread_id").unwrap_or_default(),
                        ReplayOptions {
                            after: c.number("starting_after")?,
                            follow: c.number("follow")?.unwrap_or(false),
                        },
                    )
                    .await?
            };
            for frame in c.case.response.body.as_array().expect("AG-UI SSE fixture") {
                let actual = stream.next().await.expect("expected AG-UI frame")?;
                let mut expected: AgentEvent =
                    serde_json::from_value(frame["data"].clone()).unwrap();
                expected.cursor = frame["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(|id| id.parse().unwrap());
                assert_eq!(actual, expected, "{} AG-UI frame", c.case.id);
            }
            assert!(stream.next().await.is_none());
            Ok(())
        }
        "GET /api/v1/conversations/{id}" => {
            c.check(client.responses().read_conversation(c.param("id")).await)
        }
        "POST /api/v1/conversations" => {
            c.check(client.responses().create_conversation(&c.body()?).await)
        }
        "GET /api/v1/conversations/{id}/items" => c.check(
            client
                .responses()
                .list_items(
                    c.param("id"),
                    &ItemOptions {
                        after: c.q("after"),
                        limit: c.number("limit")?,
                    },
                )
                .await,
        ),
        "GET /api/v1/responses/{response_id}" => {
            c.check(client.responses().read(c.param("response_id")).await)
        }
        "DELETE /api/v1/responses/{response_id}" => {
            c.check(client.responses().delete(c.param("response_id")).await)
        }
        "POST /api/v1/responses/{response_id}/cancel" => {
            c.check(client.responses().cancel(c.param("response_id")).await)
        }
        "POST /api/v1/responses" => {
            let body = c.body()?;
            let mut stream = if let Some(key) = c.case.request.headers.get("idempotency-key") {
                client.responses().create_with_key(&body, key).await?
            } else {
                client.responses().create(&body).await?
            };
            for frame in c
                .case
                .response
                .body
                .as_array()
                .expect("Responses SSE fixture")
            {
                let actual = stream.next().await.expect("expected Responses frame")?;
                let expected = serde_json::from_value(frame["data"].clone()).unwrap();
                assert_eq!(actual, expected, "{} Responses frame", c.case.id);
            }
            if c.case.response.stream_prefix {
                assert!(matches!(
                    stream.next().await,
                    Some(Err(Error::Transport { .. }))
                ));
            }
            assert!(stream.next().await.is_none());
            Ok(())
        }
        "GET /auth/session" => {
            let actual = client.auth().read_session().await?;
            let expected = if c.case.response.body["authenticated"] == true {
                AuthSession::SignedIn(
                    serde_json::from_value::<Identity>(c.case.response.body.clone()).unwrap(),
                )
            } else {
                AuthSession::SignedOut
            };
            assert_eq!(actual, expected);
            Ok(())
        }
        "GET /auth/signup-options" => c.check(client.auth().read_signup_options().await),
        _ => panic!("unmapped experience operation {}", c.case.operation),
    }
}
