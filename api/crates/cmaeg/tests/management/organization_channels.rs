use super::binary_support::{free_address, spawn_ready, MountedSecrets};
use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn organization_channel_crud_and_source_checks_require_current_owner_admin_evidence() {
    let database =
        database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    nyxid
        .oauth
        .grant("org-channel", "Organization", "alice", "admin");
    nyxid
        .oauth
        .grant("org-channel", "Organization", "bob", "member");
    nyxid
        .oauth
        .grant("org-other", "Other organization", "bob", "admin");
    let (alice, _) = nyxid.oauth.issue_tokens("alice");
    let (bob, _) = nyxid.oauth.issue_tokens("bob");
    let files = MountedSecrets::new();
    let address = free_address();
    let base = format!("http://{address}");
    let mut command = files.command(&address, &cma, &nyxid, &provider);
    database.configure(&mut command);
    command.env("CMAEG_GITHUB_ENABLED", "false");
    let mut child = spawn_ready(command, &address, &files.dir.join("organization.log")).await;
    let client = Client::new();
    let mut body = create_body();
    let target = json!({"type":"existing_agent","agent_id":"agt_organization"});
    body["provider"]["target"] = target.clone();
    body["profile"]["metadata"]["owner"] = json!({"kind":"organization","subject":"org-channel"});
    assert_eq!(
        client
            .post(format!("{base}/v1/channels"))
            .bearer_auth(&bob)
            .header("X-CMAEG-Owner", "org-channel")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let created = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth(&alice)
        .header("X-CMAEG-Owner", "org-channel")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = created.status();
    let saved: serde_json::Value = created.json().await.unwrap();
    assert_eq!(status, 201, "{saved}");
    assert_eq!(
        saved["definition"]["owner"],
        json!({"kind":"organization","subject":"org-channel"})
    );
    assert_eq!(saved["definition"]["provider"]["target"], target);
    let requests = provider.requests();
    let binding = requests
        .iter()
        .find(|request| request.method == "PUT" && !request.path.contains("conversations"))
        .unwrap();
    let forwarded: serde_json::Value = serde_json::from_str(&binding.body).unwrap();
    assert_eq!(forwarded["target"], target);
    assert_eq!(
        forwarded["profile"]["metadata"]["owner"],
        json!({"kind":"organization","subject":"org-channel"})
    );
    let id = saved["channel_id"].as_str().unwrap();
    let url = format!("{base}/v1/channels/{id}");
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&bob)
            .header("X-CMAEG-Owner", "org-other")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    for owner in ["", "org channel"] {
        assert_eq!(
            client
                .get(&url)
                .bearer_auth(&alice)
                .header("X-CMAEG-Owner", owner)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&alice)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&bob)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&bob)
            .header("X-CMAEG-Owner", "org-channel")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&alice)
            .header("X-CMAEG-Owner", "org-channel")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let listing = client
        .get(format!("{base}/v1/channels"))
        .bearer_auth(&alice)
        .header("X-CMAEG-Owner", "org-channel")
        .send()
        .await
        .unwrap();
    assert_eq!(listing.status(), 200);
    let listing: serde_json::Value = listing.json().await.unwrap();
    assert_eq!(listing["items"].as_array().unwrap().len(), 1);
    assert_eq!(listing["items"][0]["record_id"], saved["record_id"]);
    assert_eq!(
        listing["items"][0]["definition"]["owner"],
        saved["definition"]["owner"]
    );
    let personal: serde_json::Value = client
        .get(format!("{base}/v1/channels"))
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(personal["items"].as_array().unwrap().is_empty());
    let checked = client
        .post(format!("{url}/sources/0/test"))
        .bearer_auth(&alice)
        .header("X-CMAEG-Owner", "org-channel")
        .send()
        .await
        .unwrap();
    assert_eq!(checked.status(), 200);
    assert_eq!(
        client
            .post(format!("{url}/sources/0/test"))
            .bearer_auth(&bob)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    body.as_object_mut().unwrap().remove("credentials");
    body["expected_version"] = saved["version"].clone();
    body["name"] = "Organization channel".into();
    assert_eq!(
        client
            .put(&url)
            .bearer_auth(&bob)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let updated = client
        .put(&url)
        .bearer_auth(&alice)
        .header("X-CMAEG-Owner", "org-channel")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(updated.status(), 200);
    let updated: serde_json::Value = updated.json().await.unwrap();
    let mut activity = client
        .get(format!("{url}/activity/stream"))
        .bearer_auth(&alice)
        .header("X-CMAEG-Owner", "org-channel")
        .send()
        .await
        .unwrap();
    assert_eq!(activity.status(), 200);
    assert!(activity.chunk().await.unwrap().is_some());
    // The one-second refresh must preserve the selected owner while it is authorized.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(2200), activity.chunk())
            .await
            .is_err(),
        "organization activity stream closed during authorized refresh"
    );
    nyxid
        .oauth
        .grant("org-channel", "Organization", "alice", "member");
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), activity.chunk())
            .await
            .unwrap()
            .unwrap()
            .is_none(),
        "revoked organization admin retained activity access"
    );
    assert_eq!(
        client
            .post(format!("{url}/sources/0/test"))
            .bearer_auth(&alice)
            .header("X-CMAEG-Owner", "org-channel")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    nyxid
        .oauth
        .grant("org-channel", "Organization", "alice", "admin");
    let delete_url = format!("{url}?expected_version={}", updated["version"]);
    assert_eq!(
        client
            .delete(&delete_url)
            .bearer_auth(&bob)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .delete(&delete_url)
            .bearer_auth(&alice)
            .header("X-CMAEG-Owner", "org-channel")
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&alice)
            .header("X-CMAEG-Owner", "org-channel")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    child.kill().await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
