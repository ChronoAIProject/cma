//! Cookies are ordered header values, never comma-separated map entries.
use super::ResponseFixture;
use axum::http::{header, HeaderMap};
use std::collections::BTreeMap;

pub(super) struct ResponseHeaders {
    pub headers: BTreeMap<String, String>,
    pub set_cookies: Vec<String>,
}
impl ResponseHeaders {
    pub fn read(headers: &HeaderMap) -> Self {
        let set_cookies = headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect();
        let mut values = BTreeMap::new();
        for (name, value) in headers {
            if name == header::SET_COOKIE {
                continue;
            }
            assert!(
                values
                    .insert(name.as_str().to_owned(), value.to_str().unwrap().to_owned())
                    .is_none(),
                "single-value header fixtures cannot represent repeated {name}"
            );
        }
        values.remove("content-length");
        Self {
            headers: values,
            set_cookies,
        }
    }
}
impl ResponseFixture {
    pub(super) fn expected_headers(&self) -> ResponseHeaders {
        let mut headers = self.headers.clone();
        // Existing scalar fixtures still mean exactly one cookie. A fixture
        // cannot use both representations or conceal a repeated value in a map.
        let legacy: Vec<_> = headers
            .keys()
            .filter(|key| key.eq_ignore_ascii_case("set-cookie"))
            .cloned()
            .collect();
        assert!(legacy.len() <= 1, "duplicate cookie header names");
        let set_cookies = if let Some(name) = legacy.first() {
            assert!(
                self.set_cookies.is_empty(),
                "cookie fixture mixes headers and setCookies"
            );
            vec![headers.remove(name).unwrap()]
        } else {
            self.set_cookies.clone()
        };
        ResponseHeaders {
            headers,
            set_cookies,
        }
    }
}

#[test]
fn repeated_response_headers_cannot_hide_earlier_values() {
    for name in ["cache-control", "location"] {
        let mut headers = HeaderMap::new();
        headers.append(name, "unexpected=earlier".parse().unwrap());
        headers.append(name, "expected=last".parse().unwrap());
        assert!(std::panic::catch_unwind(|| ResponseHeaders::read(&headers)).is_err());
    }
}
#[test]
fn single_value_response_headers_keep_exact_values_except_derived_length() {
    let mut headers = HeaderMap::new();
    headers.insert("set-cookie", "expected=last".parse().unwrap());
    headers.insert("content-length", "0".parse().unwrap());
    let actual = ResponseHeaders::read(&headers);
    assert!(actual.headers.is_empty());
    assert_eq!(actual.set_cookies, ["expected=last"]);
}
#[test]
fn repeated_cookies_keep_order_duplicates_and_embedded_commas() {
    let mut headers = HeaderMap::new();
    for cookie in COOKIES {
        headers.append(header::SET_COOKIE, cookie.parse().unwrap());
    }
    let actual = ResponseHeaders::read(&headers);
    assert_eq!(actual.set_cookies, COOKIES);
    assert!(actual.headers.is_empty());
}

const COOKIES: [&str; 3] = [
    "same=first; Expires=Wed, 11 Nov 2026 12:00:00 GMT; HttpOnly",
    "same=last; Path=/; HttpOnly",
    "same=last; Path=/; HttpOnly",
];
fn response_fixture(cookies: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"status":200,"headers":{},"body":null,"bodyKind":"empty","setCookies":cookies})
}
#[test]
fn fixture_cookies_are_optional_ordered_and_strictly_typed() {
    let value = response_fixture(serde_json::json!(COOKIES));
    let parsed: ResponseFixture = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(parsed.set_cookies, COOKIES);
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    let empty: ResponseFixture =
        serde_json::from_value(response_fixture(serde_json::json!([]))).unwrap();
    let serialized = serde_json::to_value(empty).unwrap();
    assert!(serialized.get("setCookies").is_none());
    assert!(serde_json::from_value::<ResponseFixture>(serialized)
        .unwrap()
        .set_cookies
        .is_empty());
    for malformed in [
        serde_json::Value::Null,
        serde_json::json!("a=1"),
        serde_json::json!([1]),
    ] {
        assert!(serde_json::from_value::<ResponseFixture>(response_fixture(malformed)).is_err());
    }
    let mut unknown = value;
    unknown["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ResponseFixture>(unknown).is_err());
}
#[test]
fn scalar_cookie_fixtures_remain_exact_and_cannot_mix_representations() {
    let mut value = response_fixture(serde_json::json!([]));
    value["headers"] = serde_json::json!({"set-cookie":"legacy=one"});
    let parsed: ResponseFixture = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(parsed.expected_headers().set_cookies, ["legacy=one"]);
    value["setCookies"] = serde_json::json!(["extra=two"]);
    let parsed: ResponseFixture = serde_json::from_value(value).unwrap();
    assert!(std::panic::catch_unwind(|| parsed.expected_headers()).is_err());
}

fn scenario() -> super::Scenario {
    use axum::{body::Body, http::Response, routing::get, Router};
    super::Scenario {
        router: Router::new().route(
            "/cookie-proof",
            get(|| async {
                let mut response = Response::new(Body::empty());
                for cookie in COOKIES {
                    response
                        .headers_mut()
                        .append(header::SET_COOKIE, cookie.parse().unwrap());
                }
                response
            }),
        ),
        subject: None,
        authorization: layer2_core::Authorization::none(),
        symbols: BTreeMap::new(),
    }
}
fn case(cookies: Vec<String>) -> super::Case {
    serde_json::from_value(serde_json::json!({"id":"ordered-cookie-proof","operation":"GET /cookie-proof","scenario":"cookie-proof",
        "request":{"method":"GET","path":"/cookie-proof","headers":{}},"response":response_fixture(serde_json::json!(cookies))})).unwrap()
}
#[tokio::test]
async fn replay_records_and_compares_each_cookie_without_loss() {
    let mut fixture = case(vec![]);
    super::replay::run(
        &scenario(),
        &mut super::Symbols::default(),
        &mut fixture,
        true,
    )
    .await;
    assert_eq!(fixture.response.set_cookies, COOKIES);
    assert!(!fixture.response.headers.contains_key("set-cookie"));
    let encoded = serde_json::to_value(&fixture).unwrap();
    assert_eq!(
        encoded["response"]["setCookies"],
        serde_json::json!(COOKIES)
    );
    let mut decoded = serde_json::from_value(encoded).unwrap();
    super::replay::run(
        &scenario(),
        &mut super::Symbols::default(),
        &mut decoded,
        false,
    )
    .await;
}
#[tokio::test]
async fn replay_rejects_missing_extra_reordered_changed_and_folded_cookies() {
    use futures_util::FutureExt;
    let exact = COOKIES.map(str::to_owned).to_vec();
    let mut reversed = exact.clone();
    reversed.reverse();
    let mut changed = exact.clone();
    changed[0] += "; Secure";
    let mut extra = exact.clone();
    extra.push("additional=value".into());
    for expected in [
        vec![],
        exact[..2].to_vec(),
        extra,
        reversed,
        changed,
        vec![exact.join(", ")],
    ] {
        let mut fixture = case(expected);
        let mut symbols = super::Symbols::default();
        let scenario = scenario();
        let failure = std::panic::AssertUnwindSafe(super::replay::run(
            &scenario,
            &mut symbols,
            &mut fixture,
            false,
        ))
        .catch_unwind()
        .await;
        assert!(failure.is_err(), "cookie mismatch must never pass replay");
    }
}
