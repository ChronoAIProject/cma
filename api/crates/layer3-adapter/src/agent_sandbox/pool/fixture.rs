use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

#[derive(Default)]
pub(super) struct State {
    pub rows: BTreeMap<String, Value>,
    pub calls: Vec<(String, Value)>,
    pub lose: Vec<String>,
    pub conflict: Vec<String>,
    pub list_replies: VecDeque<(u16, Value)>,
    pub list_queries: Vec<String>,
    pub assignments: Vec<crate::runtime_bootstrap::RuntimeBootstrapRequest>,
    pub fail_exec: bool,
    pub exec_mutations: Vec<(String, String, Value)>,
    pub native_annotation_generation: bool,
    version: u64,
}

pub(super) struct Fixture {
    pub client: AgentSandboxPools,
    pub kubernetes: kube::Client,
    pub state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    pub async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State::default()));
        state.lock().unwrap().rows.insert(seed_path().into(),json!({
            "apiVersion":"snapshot.storage.k8s.io/v1","kind":"VolumeSnapshot",
            "metadata":{"name":"seed","namespace":"cma-test","uid":"seed-uid","annotations":{"cma/home-seed-format":"1"}},
            "status":{"readyToUse":true}
        }));
        let owned = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut preview = vec![0; 8192];
                let length = loop {
                    let count = socket.peek(&mut preview).await.unwrap();
                    assert_ne!(count, 0);
                    if preview[..count].windows(4).any(|v| v == b"\r\n\r\n") {
                        break count;
                    }
                    tokio::task::yield_now().await;
                };
                if String::from_utf8_lossy(&preview[..length])
                    .lines()
                    .next()
                    .unwrap()
                    .contains("/exec?")
                {
                    super::fixture_exec::serve(socket, &owned).await;
                    continue;
                }
                let mut input = Vec::new();
                let (end, length) = loop {
                    let mut buffer = [0; 8192];
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    input.extend_from_slice(&buffer[..count]);
                    if let Some(end) = input.windows(4).position(|v| v == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&input[..end]);
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while input.len() < end + length {
                    let mut buffer = [0; 8192];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    input.extend_from_slice(&buffer[..count]);
                }
                let head = String::from_utf8_lossy(&input[..end]);
                let mut line = head.lines().next().unwrap().split_whitespace();
                let method = line.next().unwrap();
                let path = line.next().unwrap();
                let body = if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&input[end..end + length]).unwrap()
                };
                let (status, body, lose) = respond(&owned, method, path, body);
                if lose {
                    continue;
                }
                let bytes = body.to_string();
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bytes}",bytes.len()).as_bytes()).await.unwrap();
            }
        });
        let kubernetes = kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        let client = AgentSandboxPools::new(kubernetes.clone(), super::super::tests::config());
        Self {
            client,
            kubernetes,
            state,
            task,
        }
    }

    pub fn lost(&self, method: &str, resource: &str) {
        self.state
            .lock()
            .unwrap()
            .lose
            .push(format!("{method} {}/{}", prefix(), resource));
    }

    pub fn edit(&self, resource: &str, name: &str, mutate: impl FnOnce(&mut Value)) {
        let mut state = self.state.lock().unwrap();
        mutate(
            state
                .rows
                .get_mut(&format!("{}/{resource}/{name}", prefix()))
                .unwrap(),
        );
    }

    pub fn row(&self, resource: &str, name: &str) -> Value {
        self.state.lock().unwrap().rows[&format!("{}/{resource}/{name}", prefix())].clone()
    }
}

fn respond(state: &Mutex<State>, method: &str, path: &str, body: Value) -> (u16, Value, bool) {
    let mut state = state.lock().unwrap();
    let url = reqwest::Url::parse(&format!("http://fixture{path}")).unwrap();
    let path = url.path();
    state.calls.push((format!("{method} {path}"), body.clone()));
    let call = format!("{method} {path}");
    let lost = state
        .lose
        .iter()
        .position(|pattern| call.starts_with(pattern));
    if let Some(index) = lost {
        state.lose.remove(index);
    }
    let success = json!({"apiVersion":"v1","kind":"Status","status":"Success"});
    let missing = json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","message":"missing","code":404});
    let conflict = json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Conflict","message":"conflict","code":409});
    let (status, result) = match method {
        "GET"
            if path == format!("{}/sandboxwarmpools", prefix())
                || path == format!("{}/sandboxclaims", prefix()) =>
        {
            state
                .list_queries
                .push(url.query().unwrap_or_default().into());
            if let Some((status, body)) = state.list_replies.pop_front() {
                return (status, body, lost.is_some());
            }
            let selector = url
                .query_pairs()
                .find(|(k, _)| k == "labelSelector")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let items: Vec<_> = state
                .rows
                .iter()
                .filter(|(key, value)| {
                    key.starts_with(&format!("{path}/"))
                        && selector
                            .split(',')
                            .filter(|term| !term.is_empty())
                            .all(|term| {
                                let (key, expected) = term.split_once('=').unwrap();
                                value["metadata"]["labels"][key] == expected
                            })
                })
                .map(|(_, v)| v.clone())
                .collect();
            (
                200,
                json!({"apiVersion":API_VERSION,"kind":if path.ends_with("/sandboxclaims") {"SandboxClaimList"} else {"SandboxWarmPoolList"},
                "metadata":{"resourceVersion":state.version.to_string()},"items":items}),
            )
        }
        "GET" if path == "/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes" => {
            let selector = url
                .query_pairs()
                .find(|(k, _)| k == "fieldSelector")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let items: Vec<_> = state
                .rows
                .iter()
                .filter(|(key, value)| {
                    key.starts_with(&format!("{path}/"))
                        && selector
                            .strip_prefix("metadata.name=")
                            .is_none_or(|name| value["metadata"]["name"] == name)
                })
                .map(|(_, v)| v.clone())
                .collect();
            (
                200,
                json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"SandboxList","metadata":{"resourceVersion":state.version.to_string()},"items":items}),
            )
        }
        "GET" if path == "/api/v1/namespaces/cma-test/pods" => {
            let selector = url
                .query_pairs()
                .find(|(k, _)| k == "labelSelector")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let items: Vec<_> = state
                .rows
                .iter()
                .filter(|(key, value)| {
                    key.starts_with(&format!("{path}/"))
                        && selector
                            .split(',')
                            .filter(|term| !term.is_empty())
                            .all(|term| {
                                let (key, expected) = term.split_once('=').unwrap();
                                value["metadata"]["labels"][key] == expected
                            })
                })
                .map(|(_, v)| v.clone())
                .collect();
            (
                200,
                json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":state.version.to_string()},"items":items}),
            )
        }
        "GET" if path == "/api/v1/namespaces/cma-test/persistentvolumeclaims" => {
            state
                .list_queries
                .push(url.query().unwrap_or_default().into());
            if let Some((status, body)) = state.list_replies.pop_front() {
                return (status, body, lost.is_some());
            }
            let selector = url
                .query_pairs()
                .find(|(k, _)| k == "labelSelector")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let items: Vec<_> = state
                .rows
                .iter()
                .filter(|(key, value)| {
                    key.starts_with(&format!("{path}/"))
                        && selector
                            .split(',')
                            .filter(|term| !term.is_empty())
                            .all(|term| {
                                let (key, expected) = term.split_once('=').unwrap();
                                value["metadata"]["labels"][key] == expected
                            })
                })
                .map(|(_, v)| v.clone())
                .collect();
            (
                200,
                json!({"apiVersion":"v1","kind":"PersistentVolumeClaimList","metadata":{"resourceVersion":state.version.to_string()},"items":items}),
            )
        }
        "GET" => state
            .rows
            .get(path)
            .cloned()
            .map(|v| (200, v))
            .unwrap_or((404, missing)),
        "POST" => {
            let key = format!("{path}/{}", body["metadata"]["name"].as_str().unwrap());
            if state.rows.contains_key(&key) {
                (409, conflict)
            } else {
                state.version += 1;
                let mut row = body;
                row["metadata"]["uid"] = json!(format!("uid-{}", state.version));
                row["metadata"]["resourceVersion"] = json!(state.version.to_string());
                row["metadata"]["generation"] = json!(1);
                state.rows.insert(key, row.clone());
                (201, row)
            }
        }
        "PATCH" => {
            let current = state.rows.get(path).unwrap();
            assert_eq!(body["metadata"]["uid"], current["metadata"]["uid"]);
            assert_eq!(
                body["metadata"]["resourceVersion"],
                current["metadata"]["resourceVersion"]
            );
            if let Some(index) = state.conflict.iter().position(|pending| pending == &call) {
                let version = current["metadata"]["resourceVersion"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                state.conflict.remove(index);
                state.version = state.version.max(version) + 1;
                let fresh_version = state.version.to_string();
                state.rows.get_mut(path).unwrap()["metadata"]["resourceVersion"] =
                    json!(fresh_version);
                return (409, conflict, false);
            }
            let mut next = current.clone();
            merge(&mut next, &body);
            state.version += 1;
            next["metadata"]["resourceVersion"] = json!(state.version.to_string());
            if body.get("spec").is_some()
                || (state.native_annotation_generation
                    && path.ends_with("/sandboxes/warm-member")
                    && body["metadata"]["annotations"]
                        .get(NATIVE_HOME_ANNOTATION)
                        .is_some())
            {
                next["metadata"]["generation"] =
                    json!(next["metadata"]["generation"].as_u64().unwrap() + 1);
            }
            state.rows.insert(path.into(), next.clone());
            (200, next)
        }
        "DELETE" => {
            let current = state.rows.get(path).unwrap();
            assert_eq!(body["preconditions"]["uid"], current["metadata"]["uid"]);
            assert_eq!(
                body["preconditions"]["resourceVersion"],
                current["metadata"]["resourceVersion"]
            );
            if path.starts_with(prefix()) {
                assert_eq!(body["propagationPolicy"], "Foreground");
            }
            state.rows.remove(path);
            (200, success)
        }
        _ => panic!("unexpected fixture HTTP method"),
    };
    (status, result, lost.is_some())
}

fn merge(actual: &mut Value, patch: &Value) {
    if let Some(fields) = patch.as_object() {
        for (key, value) in fields {
            merge(&mut actual[key], value);
        }
    } else {
        *actual = patch.clone();
    }
}

pub(super) fn prefix() -> &'static str {
    "/apis/extensions.agents.x-k8s.io/v1beta1/namespaces/cma-test"
}
pub(super) fn seed_path() -> &'static str {
    "/apis/snapshot.storage.k8s.io/v1/namespaces/cma-test/volumesnapshots/seed"
}
