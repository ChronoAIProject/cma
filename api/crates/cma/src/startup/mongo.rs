//! Real daemon composition with disposable Mongo and a loopback native API.
use super::{process, SyntheticKey};
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json, Router};
use futures_util::FutureExt;
use mongodb::bson::{doc, Document};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum Boundary {
    Serving,
    SocketBind,
    RuntimeFailure,
    RuntimeDelay,
    PoolDelay,
}

struct Native {
    boundary: Boundary,
    inventories: AtomicUsize,
    blocked: tokio::sync::Notify,
}

async fn native(
    State(state): State<Arc<Native>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let path = request.uri().path();
    if request.method() != axum::http::Method::GET {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"kind":"Status", "apiVersion":"v1", "status":"Failure", "reason":"Forbidden", "code":403}))).into_response();
    }
    let runtime_boundary =
        path.ends_with("/sandboxes") && state.inventories.fetch_add(1, Ordering::SeqCst) >= 1;
    if runtime_boundary && matches!(state.boundary, Boundary::RuntimeFailure) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"kind":"Status", "apiVersion":"v1", "status":"Failure", "reason":"Forbidden", "code":403}))).into_response();
    }
    if (runtime_boundary && matches!(state.boundary, Boundary::RuntimeDelay))
        || (path.ends_with("/sandboxwarmpools") && matches!(state.boundary, Boundary::PoolDelay))
    {
        state.blocked.notify_one();
        std::future::pending::<()>().await;
    }
    Json(serde_json::json!({"apiVersion":"v1", "kind":"List", "metadata":{"resourceVersion":"1"}, "items":[]}))
        .into_response()
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn daemon_startup_boundaries_never_advertise_before_serving_and_drain_after_sigterm() {
    for boundary in [
        Boundary::SocketBind,
        Boundary::RuntimeFailure,
        Boundary::RuntimeDelay,
        Boundary::PoolDelay,
        Boundary::Serving,
    ] {
        run(boundary).await;
    }
}

async fn run(boundary: Boundary) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let database = client.database(&format!("cma_startup_{}", uuid::Uuid::new_v4().simple()));
    // Exercise the production authority check with credentials scoped to this
    // disposable database. Neither inherited credentials nor an auth bypass
    // enter the daemon process.
    let username = format!("startup_{}", uuid::Uuid::new_v4().simple());
    let password = uuid::Uuid::new_v4().simple().to_string();
    database
        .run_command(doc! {
            "createUser": &username, "pwd": &password,
            "roles": [{"role":"dbOwner", "db":database.name()}],
        })
        .await
        .unwrap();
    let mut authenticated = url::Url::parse(&uri).unwrap();
    authenticated.set_username(&username).unwrap();
    authenticated.set_password(Some(&password)).unwrap();
    let query: Vec<_> = authenticated
        .query_pairs()
        .filter(|(key, _)| key != "authSource")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    authenticated
        .query_pairs_mut()
        .clear()
        .extend_pairs(query)
        .append_pair("authSource", database.name());
    let members = database.collection::<Document>("cma_agent_manager_members");
    let owners = database.collection::<Document>("cma_agent_manager_owners");
    let state = Arc::new(Native {
        boundary,
        inventories: AtomicUsize::new(0),
        blocked: tokio::sync::Notify::new(),
    });
    let native_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let native_address = format!("http://{}", native_listener.local_addr().unwrap());
    let router = Router::new().fallback(native).with_state(state.clone());
    let server = tokio::spawn(async {
        axum::serve(native_listener, router).await.unwrap();
    });
    let key = SyntheticKey::new();
    let kubeconfig = SyntheticKey::new();
    std::fs::write(&kubeconfig.0, serde_json::to_vec(&serde_json::json!({
        "apiVersion":"v1", "kind":"Config", "current-context":"synthetic",
        "clusters":[{"name":"synthetic", "cluster":{"server":native_address}}],
        "contexts":[{"name":"synthetic", "context":{"cluster":"synthetic", "user":"synthetic"}}],
        "users":[{"name":"synthetic", "user":{}}]
    })).unwrap()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let occupied = if matches!(boundary, Boundary::SocketBind) {
        Some(listener)
    } else {
        drop(listener);
        None
    };
    let mut command = process::command(authenticated.as_str(), database.name(), &key.0);
    command
        .env("KUBECONFIG", &kubeconfig.0)
        .env("CHRONO_SERVER_BIND", address.to_string())
        .env("CHRONO_SANDBOX_PROVIDER", "agent-sandbox")
        .env("CHRONO_RETAIN_OPENSANDBOX", "false")
        .env("CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE", &key.0)
        .env("CHRONO_AGENT_SANDBOX_HOME_MODE", "native_vct_full_home_v2")
        .env(
            "CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS",
            "synthetic-storage",
        )
        .env("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "1")
        .env(
            "CHRONO_NATIVE_WARM_POOL_TARGET",
            if matches!(boundary, Boundary::PoolDelay) {
                "1"
            } else {
                "0"
            },
        )
        .env(
            "CMA_TEST_STARTUP_OUTCOME",
            match boundary {
                Boundary::Serving => "serving",
                Boundary::SocketBind => "cannot bind",
                Boundary::RuntimeFailure => "sandbox runtime initialization failed",
                _ => "termination requested during startup",
            },
        );
    let mut child = command.spawn().unwrap();
    let observations = std::panic::AssertUnwindSafe(async {
        match boundary {
            Boundary::RuntimeDelay | Boundary::PoolDelay => {
                tokio::time::timeout(Duration::from_secs(60), state.blocked.notified())
                    .await
                    .unwrap();
                assert_eq!(
                    members.count_documents(doc! {}).await.unwrap(),
                    0,
                    "{boundary:?}"
                );
                assert_eq!(
                    owners.count_documents(doc! {}).await.unwrap(),
                    0,
                    "{boundary:?}"
                );
                terminate(&child);
            }
            Boundary::Serving => {
                let http = reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(1))
                    .build()
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(60), async {
                    loop {
                        if let Some(status) = child.try_wait().unwrap() {
                            panic!("daemon exited before readiness: {status}");
                        }
                        if let Ok(response) =
                            http.get(format!("http://{address}/ready")).send().await
                        {
                            if response.status() == StatusCode::OK {
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(
                    members
                        .count_documents(doc! {"draining":false})
                        .await
                        .unwrap(),
                    1
                );
                assert_eq!(owners.count_documents(doc! {}).await.unwrap(), 0);
                terminate(&child);
            }
            _ => (),
        }
        tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
            .await
            .unwrap()
            .unwrap()
    })
    .catch_unwind()
    .await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    drop(occupied);
    let member_rows = members.count_documents(doc! {}).await.unwrap();
    let admitting = members
        .count_documents(doc! {"draining":false})
        .await
        .unwrap();
    let owner_rows = owners.count_documents(doc! {}).await.unwrap();
    database
        .run_command(doc! {"dropUser": &username})
        .await
        .unwrap();
    database.drop().await.unwrap();
    let observations = observations.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    assert!(
        observations.status.success(),
        "{boundary:?}: {} {}",
        String::from_utf8_lossy(&observations.stdout),
        String::from_utf8_lossy(&observations.stderr)
    );
    assert_eq!(owner_rows, 0, "{boundary:?}");
    assert_eq!(admitting, 0, "{boundary:?}");
    assert_eq!(
        member_rows,
        usize::from(matches!(boundary, Boundary::Serving)) as u64,
        "{boundary:?}"
    );
}

fn terminate(child: &tokio::process::Child) {
    assert!(std::process::Command::new("kill")
        .args(["-TERM", &child.id().unwrap().to_string()])
        .status()
        .unwrap()
        .success());
}
