use super::context::Context;
use cma_sdk::{
    Result,
    bridges::{BridgeListOptions, ImportBridgeKeyRequest},
};

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let bridges = c.client.bridges();
    match c.case.operation.as_str() {
        "GET /api/v1/bridges/{id}/deliveries" => {
            c.check(bridges.list_deliveries(c.param("id"), &c.page()?).await)
        }
        "GET /api/v1/bridges/{id}/deliveries/{item}" => {
            c.check(bridges.read_delivery(c.param("id"), c.param("item")).await)
        }
        "POST /api/v1/bridges/{id}/delivery-recovery" => c.check(
            bridges
                .recover_delivery(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/bridges/{id}/invoke-grants" => {
            c.check(bridges.list_invoke_grants(c.param("id"), &c.page()?).await)
        }
        "GET /api/v1/bridges/{id}/invoke-readiness" => {
            c.check(bridges.invoke_readiness(c.param("id")).await)
        }
        "GET /api/v1/bridge-invoke-grants/{id}" => {
            c.check(bridges.read_invoke_grant(c.param("id")).await)
        }
        "POST /api/v1/bridges/{id}/invoke-grants" => c.check(
            bridges
                .issue_invoke_grant(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "POST /api/v1/bridge-invoke-grants/{id}/revoke" => c.check(
            bridges
                .revoke_invoke_grant(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/bridge-connectors/capabilities" => c.check(
            bridges
                .capabilities(c.query.get("organization").map(String::as_str))
                .await,
        ),
        "GET /api/v1/bridge-connectors" => c.check(
            bridges
                .choices(c.query.get("organization").map(String::as_str))
                .await,
        ),
        "GET /api/v1/bridges" => c.check(
            bridges
                .list(&BridgeListOptions {
                    organization: c.q("organization"),
                    page: c.page()?,
                })
                .await,
        ),
        "GET /api/v1/bridges/{id}" => c.check(bridges.read(c.param("id")).await),
        "GET /api/v1/bridges/{id}/configuration" => {
            c.check(bridges.configuration(c.param("id")).await)
        }
        "POST /api/v1/bridges" => c.check(bridges.create(&c.body()?, &c.key()?).await),
        "PUT /api/v1/bridges/{id}/configuration" => {
            c.check(bridges.update(c.param("id"), &c.body()?, &c.key()?).await)
        }
        "POST /api/v1/bridges/{id}/commands" => {
            c.check(bridges.control(c.param("id"), &c.body()?, &c.key()?).await)
        }
        "POST /api/v1/bridges/{id}/validate" => c.check(
            bridges
                .validate(c.param("id"), c.field("expected_revision")?)
                .await,
        ),
        "PUT /api/v1/bridges/{id}/credential" => {
            let key: String = c.field("key")?;
            c.check(
                bridges
                    .import_key(
                        c.param("id"),
                        &ImportBridgeKeyRequest {
                            expected_revision: c.field("expected_revision")?,
                            key: &key,
                        },
                    )
                    .await,
            )
        }
        "POST /api/v1/bridges/{id}/credential/revoke" => c.check(
            bridges
                .revoke_key(
                    c.param("id"),
                    c.field("expected_revision")?,
                    c.field("expected_custody_revision")?,
                    &c.key()?,
                )
                .await,
        ),
        _ => panic!("unmapped bridge operation {}", c.case.operation),
    }
}
