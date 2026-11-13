use super::context::Context;
use cma_sdk::{Result, documents::DocumentListOptions};
pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let api = c.client.documents();
    let token = c
        .case
        .request
        .headers
        .get("x-cma-document-share-token")
        .map(String::as_str);
    match c.case.operation.as_str() {
        "GET /api/v1/documents" => c.check(
            api.list(&DocumentListOptions {
                limit: c.number("limit")?,
                cursor: c.q("cursor"),
                scope: c.q("scope"),
                q: c.q("q"),
            })
            .await,
        ),
        "POST /api/v1/documents" => c.check(api.create(&c.body()?, &c.key()?).await),
        "GET /api/v1/documents/{id}" => c.check(api.read(c.param("id"), token).await),
        "PATCH /api/v1/documents/{id}" => {
            c.check(api.update(c.param("id"), &c.body()?, &c.key()?).await)
        }
        "DELETE /api/v1/documents/{id}" => {
            c.check(api.delete(c.param("id"), &c.body()?, &c.key()?).await)
        }
        "GET /api/v1/documents/{id}/source" => c.check(
            api.source(c.param("id"), c.number("revision")?, token)
                .await,
        ),
        "GET /api/v1/documents/{id}/revisions" => {
            c.check(api.revisions(c.param("id"), &c.page()?, token).await)
        }
        "GET /api/v1/documents/{id}/shares" => c.check(api.shares(c.param("id")).await),
        "PUT /api/v1/documents/{id}/shares/{subject}" => c.check(
            api.grant(c.param("id"), c.param("subject"), &c.body()?, &c.key()?)
                .await,
        ),
        "DELETE /api/v1/documents/{id}/shares/{subject}" => c.check(
            api.revoke(c.param("id"), c.param("subject"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/documents/{id}/share-links" => c.check(api.links(c.param("id")).await),
        "POST /api/v1/documents/{id}/share-links" => {
            c.check(api.create_link(c.param("id"), &c.body()?, &c.key()?).await)
        }
        "DELETE /api/v1/documents/{id}/share-links/{link_id}" => c.check(
            api.revoke_link(c.param("id"), c.param("link_id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/documents/{id}/comments" => {
            c.check(api.comments(c.param("id"), &c.page()?, token).await)
        }
        "POST /api/v1/documents/{id}/comments" => c.check(
            api.comment(c.param("id"), &c.body()?, &c.key()?, token)
                .await,
        ),
        "DELETE /api/v1/documents/{id}/comments/{comment_id}" => c.check(
            api.remove_comment(
                c.param("id"),
                c.param("comment_id"),
                &c.body()?,
                &c.key()?,
                token,
            )
            .await,
        ),
        op => panic!("missing document operation {op}"),
    }
}
