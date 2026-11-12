use super::context::Context;
use cma_sdk::Result;

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let account = c.client.account();
    let deletion = account.deletion();
    match c.case.operation.as_str() {
        "POST /api/v1/account/deletion" => c.check(deletion.request(&c.key()?).await),
        "GET /api/v1/account/deletion" => c.check(deletion.status().await),
        operation => panic!("unmapped account deletion operation {operation}"),
    }
}
