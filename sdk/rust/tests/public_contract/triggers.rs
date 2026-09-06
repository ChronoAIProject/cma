use super::context::Context;
use cma_sdk::{
    Result,
    launches::{LaunchListOptions, LaunchReplayOptions},
    triggers::{BadgeOptions, BadgeResponse},
};

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let triggers = c.client.triggers();
    let launches = c.client.launches();
    match c.case.operation.as_str() {
        "GET /api/v1/triggers" => c.check(triggers.list(&c.page()?).await),
        "POST /api/v1/triggers" => c.check(triggers.create(&c.field("content")?, &c.key()?).await),
        "GET /api/v1/triggers/{id}" => c.check(triggers.read(c.param("id")).await),
        "PUT /api/v1/triggers/{id}" => c.check(
            triggers
                .save(
                    c.param("id"),
                    c.field("draft_version")?,
                    &c.field("content")?,
                    &c.key()?,
                )
                .await,
        ),
        "DELETE /api/v1/triggers/{id}" => c.check(triggers.delete(c.param("id"), &c.key()?).await),
        "POST /api/v1/triggers/{id}/publish" => c.check(
            triggers
                .publish(c.param("id"), c.field("draft_version")?, &c.key()?)
                .await,
        ),
        "POST /api/v1/triggers/{id}/retire" => {
            c.check(triggers.retire(c.param("id"), &c.key()?).await)
        }
        "GET /api/v1/triggers/{id}/revisions" => {
            c.check(triggers.list_revisions(c.param("id")).await)
        }
        "GET /api/v1/triggers/{id}/placements" => c.check(
            triggers
                .list_placements(c.param("id"), c.number("include_retired")?.unwrap_or(false))
                .await,
        ),
        "POST /api/v1/triggers/{id}/placements" => c.check(
            triggers
                .create_placement(
                    c.param("id"),
                    &c.field::<String>("source_url")?,
                    &c.field::<String>("label")?,
                    &c.key()?,
                )
                .await,
        ),
        "POST /api/v1/triggers/{id}/placements/{placement_id}/retire" => c.check(
            triggers
                .retire_placement(c.param("id"), c.param("placement_id"), &c.key()?)
                .await,
        ),
        "GET /api/v1/triggers/{id}/embed" => c.check(
            triggers
                .embed(c.param("id"), &c.q("placement_id").unwrap_or_default())
                .await,
        ),
        "GET /api/v1/triggers/{id}/stats" => c.check(triggers.stats(c.param("id")).await),
        "GET /api/v1/triggers/public/{public_id}" => {
            c.check(c.client.read_trigger_preview(c.param("public_id")).await)
        }
        "GET /api/v1/triggers/public/{public_id}/badge.svg" => {
            let result = c
                .client
                .read_trigger_badge(
                    c.param("public_id"),
                    &BadgeOptions {
                        label: c.tag("label")?,
                        theme: c.tag("theme")?,
                        placement: c.q("placement"),
                        if_none_match: c.case.request.headers.get("if-none-match").cloned(),
                    },
                )
                .await?;
            let expected = if c.case.response.status == 304 {
                BadgeResponse::NotModified {
                    etag: c.case.response.headers.get("etag").unwrap().clone(),
                }
            } else {
                BadgeResponse::Modified {
                    svg: c.case.response.body.as_str().unwrap().into(),
                    etag: c.case.response.headers.get("etag").unwrap().clone(),
                }
            };
            assert_eq!(result, expected);
            Ok(())
        }
        "POST /api/v1/triggers/public/{public_id}/launches" => c.check(
            launches
                .create_or_resume(c.param("public_id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/launches" => c.check(
            launches
                .list(&LaunchListOptions {
                    trigger: c.q("trigger"),
                    state: c.tag("state")?,
                    page: c.page()?,
                })
                .await,
        ),
        "GET /api/v1/launches/{id}" => c.check(launches.read(c.param("id")).await),
        "POST /api/v1/launches/{id}/start" => {
            c.check(launches.start(c.param("id"), &c.body()?, &c.key()?).await)
        }
        "POST /api/v1/launches/{id}/cancel" => {
            c.check(launches.cancel(c.param("id"), &c.key()?).await)
        }
        "GET /api/v1/launches/{id}/requirements" => {
            c.check(launches.requirements(c.param("id")).await)
        }
        "GET /api/v1/launches/{id}/events" => {
            c.check(launches.events(c.param("id"), c.events()?).await)
        }
        "GET /api/v1/agui/launches/{id}/events" => {
            let mut stream = launches
                .stream(
                    c.param("id"),
                    LaunchReplayOptions {
                        after: c.number("starting_after")?,
                        limit: c.number("limit")?,
                        follow: c.number("follow")?.unwrap_or(false),
                    },
                )
                .await?;
            for frame in c.case.response.body.as_array().expect("Launch SSE fixture") {
                let actual = stream.next().await.expect("expected Launch event")?;
                let expected = serde_json::from_value(frame["data"].clone()).unwrap();
                assert_eq!(actual, expected);
            }
            assert!(stream.next().await.is_none());
            Ok(())
        }
        "POST /api/v1/service-connections" => {
            c.check(launches.create_connection(&c.body()?, &c.key()?).await)
        }
        "GET /api/v1/service-connections/{handoff_id}" => {
            c.check(launches.read_connection(c.param("handoff_id")).await)
        }
        _ => panic!("unmapped Trigger operation {}", c.case.operation),
    }
}
