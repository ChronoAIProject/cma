use super::wire::{Expected, headers};
use cma_sdk::{Client, Error, EventOptions, IdempotencyKey, PageOptions, Result};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeMap, fmt::Debug, path::PathBuf, str::FromStr};

#[derive(Clone, Debug, Deserialize)]
pub struct Case {
    pub id: String,
    pub operation: String,
    pub scenario: String,
    pub request: Request,
    pub response: Response,
    #[serde(default)]
    pub captures: BTreeMap<String, String>,
    #[serde(default, rename = "headerCaptures")]
    pub header_captures: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Request {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
    #[serde(rename = "rawBody")]
    pub raw_body: Option<String>,
    #[serde(rename = "rawBodyRepeat")]
    pub raw_body_repeat: Option<usize>,
}
impl Request {
    pub fn expanded_raw_body(&self) -> Option<String> {
        assert!(self.raw_body_repeat.is_none() || self.raw_body.is_some());
        let raw = self.raw_body.as_ref()?;
        assert!(
            self.body.is_none(),
            "body and rawBody are mutually exclusive"
        );
        let repeat = self.raw_body_repeat.unwrap_or(1);
        assert!((1..=2_097_153).contains(&repeat), "invalid rawBodyRepeat");
        let length = raw.len().checked_mul(repeat).unwrap();
        assert!(length <= 8 * 1024 * 1024, "expanded rawBody exceeds 8 MiB");
        Some(raw.repeat(repeat))
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    #[serde(rename = "bodyKind")]
    pub body_kind: String,
    #[serde(default, rename = "streamPrefix")]
    pub stream_prefix: bool,
    pub body: Value,
}

pub fn load() -> Vec<Case> {
    #[derive(Deserialize)]
    struct Corpus {
        version: u32,
        cases: Vec<Case>,
    }
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../api/crates/layer1-public/tests/fixtures/public-contract");
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|path| path.unwrap().path())
        .filter(|path| path.extension().is_some_and(|value| value == "json"))
        .collect();
    paths.sort();
    let selected = std::env::var("CMA_SDK_CONTRACT_FILE").ok();
    let mut result = Vec::new();
    for path in paths {
        if selected
            .as_ref()
            .is_some_and(|selected| path.file_name().unwrap() != selected.as_str())
        {
            continue;
        }
        let mut corpus: Corpus = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(corpus.version, 1);
        let mut scenes: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
        for case in &corpus.cases {
            for (name, header) in &case.header_captures {
                let value = Value::String(
                    case.response
                        .headers
                        .get(header)
                        .expect("receipt header capture")
                        .clone(),
                );
                if let Some(prior) = scenes
                    .entry(case.scenario.clone())
                    .or_default()
                    .insert(name.clone(), value.clone())
                {
                    assert_eq!(prior, value, "conflicting header capture {name}");
                }
            }
            for (name, pointer) in &case.captures {
                let value = case.response.body.pointer(pointer).unwrap().clone();
                if let Some(prior) = scenes
                    .entry(case.scenario.clone())
                    .or_default()
                    .insert(name.clone(), value.clone())
                {
                    assert_eq!(prior, value, "conflicting canonical symbol {name}");
                }
            }
        }
        for case in &mut corpus.cases {
            let _ = case.request.expanded_raw_body();
            let symbols = scenes.entry(case.scenario.clone()).or_default();
            symbols
                .entry("interaction_expiry".into())
                .or_insert_with(|| {
                    serde_json::json!(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs()
                            + 3600
                    )
                });
            symbols
                .entry("ephemeral_credential".into())
                .or_insert_with(|| {
                    use std::hash::{BuildHasher, Hasher};
                    let value = std::collections::hash_map::RandomState::new()
                        .build_hasher()
                        .finish();
                    Value::String(format!("runtime-{value:016x}-{value:016x}"))
                });
            case.request.path = resolve(&Value::String(case.request.path.clone()), symbols)
                .as_str()
                .unwrap()
                .into();
            for value in case.request.headers.values_mut() {
                *value = resolve(&Value::String(value.clone()), symbols)
                    .as_str()
                    .unwrap()
                    .into();
            }
            case.request.body = case
                .request
                .body
                .as_ref()
                .map(|value| resolve(value, symbols));
            case.response.body = resolve(&case.response.body, symbols);
            for value in case.response.headers.values_mut() {
                *value = resolve(&Value::String(value.clone()), symbols)
                    .as_str()
                    .unwrap()
                    .into();
            }
        }
        result.extend(corpus.cases);
    }
    assert!(!result.is_empty());
    result
}

fn resolve(value: &Value, symbols: &BTreeMap<String, Value>) -> Value {
    match value {
        Value::String(value)
            if value.starts_with('$')
                && value[1..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') =>
        {
            symbols
                .get(&value[1..])
                .unwrap_or_else(|| panic!("unknown symbol {value}"))
                .clone()
        }
        Value::String(value) => {
            let mut rest = value.as_str();
            let mut result = String::new();
            while let Some(index) = rest.find('$') {
                result.push_str(&rest[..index]);
                rest = &rest[index + 1..];
                let length = rest
                    .bytes()
                    .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                    .count();
                assert!(length > 0);
                result.push_str(
                    symbols
                        .get(&rest[..length])
                        .unwrap_or_else(|| panic!("unknown symbol {}", &rest[..length]))
                        .as_str()
                        .expect("embedded symbol is a string"),
                );
                rest = &rest[length..];
            }
            result.push_str(rest);
            Value::String(result)
        }
        Value::Array(values) => {
            Value::Array(values.iter().map(|value| resolve(value, symbols)).collect())
        }
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), resolve(value, symbols)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

pub struct Context<'a> {
    pub client: &'a Client,
    pub case: &'a Case,
    pub params: BTreeMap<String, String>,
    pub query: BTreeMap<String, String>,
}
impl<'a> Context<'a> {
    pub fn new(client: &'a Client, case: &'a Case) -> Self {
        let url = reqwest::Url::parse(&format!("http://fixture{}", case.request.path)).unwrap();
        let (_, template) = case.operation.split_once(' ').unwrap();
        let params = template
            .split('/')
            .zip(url.path().split('/'))
            .filter_map(|(pattern, value)| {
                pattern
                    .strip_prefix('{')
                    .and_then(|key| key.strip_suffix('}'))
                    .map(|key| {
                        let value = percent_encoding::percent_decode_str(value)
                            .decode_utf8()
                            .expect("fixture path parameter must be UTF-8");
                        (key.into(), value.into_owned())
                    })
            })
            .collect();
        Self {
            client,
            case,
            params,
            query: url
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect(),
        }
    }
    pub fn param(&self, name: &str) -> &str {
        self.params.get(name).unwrap()
    }
    pub fn value(&self, field: &str) -> Result<&Value> {
        self.case
            .request
            .body
            .as_ref()
            .and_then(|value| value.get(field))
            .ok_or_else(|| invalid(format!("missing body field {field}")))
    }
    pub fn body<T: DeserializeOwned>(&self) -> Result<T> {
        decode(
            self.case
                .request
                .body
                .clone()
                .ok_or_else(|| invalid("missing JSON body"))?,
        )
    }
    pub fn field<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        decode(self.value(name)?.clone())
    }
    pub fn key(&self) -> Result<IdempotencyKey> {
        IdempotencyKey::new(
            self.case
                .request
                .headers
                .get("idempotency-key")
                .cloned()
                .unwrap_or_default(),
        )
    }
    pub fn q(&self, key: &str) -> Option<String> {
        self.query.get(key).cloned()
    }
    pub fn number<T: FromStr>(&self, key: &str) -> Result<Option<T>> {
        self.q(key)
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| invalid(format!("unrepresentable numeric query {key}")))
            })
            .transpose()
    }
    pub fn tag<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.q(key)
            .map(|value| decode(Value::String(value)))
            .transpose()
    }
    pub fn page(&self) -> Result<PageOptions> {
        Ok(PageOptions {
            cursor: self.q("cursor"),
            limit: self.number("limit")?,
        })
    }
    pub fn events(&self) -> Result<EventOptions> {
        Ok(EventOptions {
            after: self.number("after")?,
            limit: self.number("limit")?,
        })
    }
    pub fn check<T: DeserializeOwned + PartialEq + Debug>(&self, result: Result<T>) -> Result<()> {
        let actual = result?;
        let mut expected = self.case.response.body.clone();
        if self.case.response.body_kind == "empty" {
            expected = Value::Null;
        }
        if matches!(
            self.case.operation.as_str(),
            "GET /api/v1/triggers" | "GET /api/v1/launches"
        ) && let Some(body) = expected.as_object_mut()
            && let Some(next) = body.remove("next_cursor")
        {
            body.insert("next".into(), next);
        }
        let expected: T = serde_json::from_value(expected)
            .unwrap_or_else(|error| panic!("{} expected typed response: {error}", self.case.id));
        assert_eq!(actual, expected, "{} typed response", self.case.id);
        Ok(())
    }
}
pub fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value)
        .map_err(|_| invalid("fixture input cannot be represented by the closed request model"))
}
pub fn invalid(message: impl Into<String>) -> Error {
    Error::Validation {
        message: format!("fixture shape: {}", message.into()),
    }
}

pub fn wire(case: &Case) -> Expected {
    let stream = case.operation.contains("/agui/")
        && (case.operation.ends_with("/events") || case.operation.ends_with("/run"))
        || case.operation == "POST /api/v1/responses"
        || case.operation.contains("/api/v2")
            && (case.operation == "POST /api/v2/responses"
                || case.operation == "POST /api/v2/agents/{agent_id}/responses"
                || case.operation.ends_with("/events"));
    let mut request_headers = headers(
        false,
        if stream {
            "text/event-stream"
        } else if case.operation.ends_with("/badge.svg") {
            "image/svg+xml"
        } else {
            "application/json"
        },
    );
    request_headers.extend(
        case.request
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone())),
    );
    let response = if case.response.body_kind == "sse" {
        case.response
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|frame| {
                let mut bytes = String::new();
                if let Some(id) = frame["id"].as_str() {
                    bytes.push_str(&format!("id: {id}\n"));
                }
                if let Some(event) = frame["event"].as_str() {
                    bytes.push_str(&format!("event: {event}\n"));
                }
                bytes.push_str(&format!("data: {}\n\n", frame["data"]));
                bytes
            })
            .collect::<String>()
            .into_bytes()
    } else if case.response.body_kind == "empty" {
        vec![]
    } else if case.response.body_kind == "text" {
        case.response.body.as_str().unwrap().as_bytes().to_vec()
    } else {
        serde_json::to_vec(&case.response.body).unwrap()
    };
    Expected {
        method: case.request.method.clone(),
        path: case.request.path.clone(),
        headers: request_headers,
        body: case.request.body.clone(),
        raw_body: case.request.expanded_raw_body().map(String::into_bytes),
        status: case.response.status,
        response_headers: case.response.headers.clone(),
        response,
        chunks: None,
    }
}
