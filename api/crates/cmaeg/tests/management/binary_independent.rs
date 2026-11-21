//! The production composition keeps Console and media enabled over Gateway Mongo.
use super::binary_support::{assert_no_secrets, free_address, spawn_ready, MountedSecrets};
use super::*;
use fake_nyxid::console as oauth;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn independent_records_preserve_console_media_and_portal_authority() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let address = free_address();
    let base = format!("http://{address}");
    let log = files.dir.join("trace.log");
    let mut command = files.command(&address, &cma, &nyxid, &provider);
    let database =
        database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
    database.configure(&mut command);
    command
        .env("CMAEG_MEDIA_ENABLED", "true")
        .env("CMAEG_MEDIA_TOOL_PROVIDERS", "cma_codex")
        .env("CMAEG_CONSOLE_ENABLED", "true")
        .env("CMAEG_CONSOLE_CLIENT_ID", oauth::CLIENT_ID)
        .env("CMAEG_CONSOLE_CLIENT_SECRET", oauth::CLIENT_SECRET);
    let mut child = spawn_ready(command, &address, &log).await;
    let http = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    // The old Agent Manager record server is unavailable for the entire journey.
    cma.close().await;
    let mut body = create_body();
    body["record_id"] = uuid::Uuid::new_v4().to_string().into();
    let created = http
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let created: serde_json::Value = created.json().await.unwrap();
    let id = created["channel_id"].as_str().unwrap();
    let version = created["version"].as_u64().unwrap();
    let record = database
        .admin
        .database(&database.name)
        .collection::<mongodb::bson::Document>("eg_channel_records")
        .find_one(mongodb::bson::doc! {"owner":"alice"})
        .await
        .unwrap()
        .unwrap();
    let record_id = record.get_str("_id").unwrap();
    let listed = http
        .get(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), 200);
    assert!(listed.text().await.unwrap().contains(id));
    assert_eq!(
        http.get(format!("{base}/v1/channels"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let tools: serde_json::Value = http
        .post(format!("{base}/mcp"))
        .bearer_auth("nyx_abcdefghijklmnop")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "readEventAttachment"));

    let cookie = sign_in(&http, &base).await;
    let channel_url = format!("{base}/console/channels/{record_id}");
    let page = http
        .get(&channel_url)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    assert_eq!(page.headers()["cache-control"], "no-store");
    assert!(page.text().await.unwrap().contains("support-bot"));
    // A Console cookie does not confer portal/API authority.
    assert_eq!(
        http.get(format!("{base}/v1/channels"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let refused = http
        .post(format!("{base}/console/owner"))
        .header("cookie", &cookie)
        .header("origin", "https://foreign.example")
        .form(&[("owner", "personal"), ("csrf", "invalid")])
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    // Revoking the durable record is visible to the signed-in Console too.
    let deleted = http
        .delete(format!(
            "{base}/v1/channels/{id}?expected_version={version}"
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    assert_eq!(
        http.get(channel_url)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    assert_no_secrets(&std::fs::read_to_string(log).unwrap());
    provider.close().await;
    nyxid.close().await;
}

async fn sign_in(http: &Client, base: &str) -> String {
    let response = http
        .get(format!("{base}/console/login"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 302);
    let login_cookie = cookie(&response, "__Host-cmaeg-console-login");
    let response = http
        .get(response.headers()["location"].to_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 303);
    let response = http
        .get(response.headers()["location"].to_str().unwrap())
        .header("cookie", login_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 303);
    cookie(&response, "__Host-cmaeg-console")
}

fn cookie(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(&format!("{name}=")))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}
