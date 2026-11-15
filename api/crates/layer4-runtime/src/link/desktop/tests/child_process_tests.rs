use super::*;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const CHILD: &str = "link::desktop::tests::child_fixture::desktop_receiver_child";

struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Child {
    process: tokio::process::Child,
    output: BufReader<tokio::process::ChildStdout>,
    address: std::net::SocketAddr,
}

impl Child {
    async fn start(root: &Path, token: &str) -> Self {
        let mut process = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                CHILD,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("CMA_DESKTOP_RECEIVER_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input =
            serde_json::json!({"root":root,"sandbox_id":"child-sandbox","control_token":token});
        process
            .stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{input}\n").as_bytes())
            .await
            .unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        let address = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut line = String::new();
                assert_ne!(
                    output.read_line(&mut line).await.unwrap(),
                    0,
                    "child exited before readiness"
                );
                assert!(
                    !line.contains(token),
                    "readiness must not disclose credentials"
                );
                if let Some((_, json)) = line.split_once("CMA_DESKTOP_RECEIVER_READY ") {
                    let ready: serde_json::Value = serde_json::from_str(json).unwrap();
                    assert_eq!(ready["protocol"], 1);
                    assert_eq!(ready["sandbox_id"], "child-sandbox");
                    let address: std::net::SocketAddr =
                        ready["address"].as_str().unwrap().parse().unwrap();
                    assert!(address.ip().is_loopback());
                    break address;
                }
            }
        })
        .await
        .expect("bounded child readiness");
        Self {
            process,
            output,
            address,
        }
    }

    async fn stop(mut self, token: &str) {
        drop(self.process.stdin.take());
        let status = tokio::time::timeout(Duration::from_secs(5), self.process.wait())
            .await
            .expect("EOF stops the receiver child")
            .unwrap();
        assert!(status.success());
        let mut tail = String::new();
        self.output.read_to_string(&mut tail).await.unwrap();
        assert!(
            !tail.contains(token),
            "child output must not disclose credentials"
        );
    }

    fn handshake(&self, token: &str, owner: Option<&DesktopOwnership>) -> axum::http::Request<()> {
        let mut request = format!("ws://{}/runtime/desktop", self.address)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("x-cma-runtime-token", token.parse().unwrap());
        if let Some(owner) = owner {
            request.headers_mut().insert(
                DESKTOP_OWNER_HEADER,
                serde_json::to_string(owner).unwrap().parse().unwrap(),
            );
        }
        request
    }

    fn control(
        &self,
        client: &reqwest::Client,
        token: &str,
        action: &str,
    ) -> reqwest::RequestBuilder {
        client
            .post(format!(
                "http://{}/runtime/desktop/ownership/{action}",
                self.address
            ))
            .header("x-cma-runtime-token", token)
    }
}

async fn accepted(child: &Child, http: &reqwest::Client, token: &str, owner: &DesktopOwnership) {
    let request = DesktopChallengeRequest {
        sandbox_id: owner.sandbox_id.clone(),
        runtime_id: owner.runtime_id.clone(),
        previous: None,
    };
    let challenge: DesktopOwnershipChallenge = child
        .control(http, token, "challenge")
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        child
            .control(http, token, "confirm")
            .json(&DesktopConfirmation {
                ownership: owner.clone(),
                nonce: challenge.nonce,
            })
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn receiver_child_authentication_eof_drain_and_restart_preserve_floor() {
    let root =
        Root(std::env::temp_dir().join(format!("cma-desktop-child-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&root.0).unwrap();
    let token = uuid::Uuid::new_v4().simple().to_string().repeat(2);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let child = Child::start(&root.0, &token).await;
    let mut owner = DesktopOwnership {
        sandbox_id: "child-sandbox".into(),
        runtime_id: "external-child-runtime".into(),
        boot_id: None,
        session_id: "first-child-session".into(),
        epoch: 1,
        issued_at_ms: 1000,
        expires_at_ms: 16000,
    };
    assert_eq!(
        child
            .control(&http, "invalid", "challenge")
            .json(&DesktopChallengeRequest {
                sandbox_id: owner.sandbox_id.clone(),
                runtime_id: owner.runtime_id.clone(),
                previous: None,
            })
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    accepted(&child, &http, &token, &owner).await;
    let (mut owned, _) = tokio_tungstenite::connect_async(child.handshake(&token, Some(&owner)))
        .await
        .unwrap();
    owned
        .send(TungsteniteMessage::Binary(vec![4, 5]))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), owned.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        TungsteniteMessage::Binary(vec![4, 5])
    );
    child.stop(&token).await;
    assert!(root.0.join("state/desktop-owner.activated").is_file());
    assert!(root.0.join("state/desktop-owner/floor.json").is_file());
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), owned.next())
            .await
            .unwrap(),
        None | Some(Err(_)) | Some(Ok(TungsteniteMessage::Close(_)))
    ));
    let child = Child::start(&root.0, &token).await;
    assert!(
        tokio_tungstenite::connect_async(child.handshake(&token, None))
            .await
            .is_err()
    );
    assert!(
        tokio_tungstenite::connect_async(child.handshake(&token, Some(&owner)))
            .await
            .is_err()
    );
    assert_eq!(
        child
            .control(&http, &token, "challenge")
            .json(&DesktopChallengeRequest {
                sandbox_id: owner.sandbox_id.clone(),
                runtime_id: owner.runtime_id.clone(),
                previous: Some(owner.clone()),
            })
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    owner.epoch = 2;
    owner.session_id = "successor-child-session".into();
    accepted(&child, &http, &token, &owner).await;
    assert_eq!(
        child
            .control(&http, &token, "revoke")
            .json(&owner)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    child.stop(&token).await;
}
