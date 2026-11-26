use super::*;
use reqwest::header::{COOKIE, LOCATION, SET_COOKIE};

pub(super) struct Attempt {
    pub path: String,
    pub binding: String,
}

pub(super) async fn begin(fixture: &HttpFixture, server: &Server, subject: &str) -> Attempt {
    let login = fixture
        .request(server, Method::GET, "/auth/login", None)
        .query(&[("return_to", "/sandboxes/contract?tab=access")])
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    assert_eq!(login.headers()["cache-control"], "no-store");
    let binding = login
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|h| h.to_str().unwrap())
        .find(|h| h.starts_with("cma_login_"))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(session(&login).is_none());
    let mut authorize = reqwest::Url::parse(login.headers()[LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(
        authorize.origin().ascii_serialization(),
        fixture.provider.server.base
    );
    authorize
        .query_pairs_mut()
        .append_pair("contract_subject", subject);
    let consent = fixture.client.get(authorize).send().await.unwrap();
    assert_eq!(consent.status(), StatusCode::SEE_OTHER);
    let callback = reqwest::Url::parse(consent.headers()[LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(
        callback.origin().ascii_serialization(),
        "https://cma.example.test"
    );
    Attempt {
        path: format!("{}?{}", callback.path(), callback.query().unwrap()),
        binding,
    }
}

pub(super) fn callback(
    fixture: &HttpFixture,
    server: &Server,
    attempt: &Attempt,
) -> reqwest::RequestBuilder {
    fixture
        .request(server, Method::GET, &attempt.path, None)
        .header(COOKIE, &attempt.binding)
}

pub(super) async fn complete(
    fixture: &HttpFixture,
    server: &Server,
    attempt: &Attempt,
    subject: &str,
) -> String {
    let reply = callback(fixture, server, attempt).send().await.unwrap();
    assert_eq!(reply.status(), StatusCode::SEE_OTHER);
    assert_eq!(reply.headers()[LOCATION], "/sandboxes/contract?tab=access");
    assert_eq!(reply.headers()["cache-control"], "no-store");
    let cookie = session(&reply).unwrap();
    assert_ne!(cookie, attempt.binding.split_once('=').unwrap().1);
    let login = fixture
        .custody
        .browser_login(&SecretDigest::identify("browser-login", &cookie))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(login.subject.as_str(), subject);
    assert!(fixture
        .custody
        .subject(&SubjectId::new(subject))
        .await
        .unwrap()
        .unwrap()
        .tokens
        .is_some());
    cookie
}

pub(super) fn session(reply: &reqwest::Response) -> Option<String> {
    reply.headers().get_all(SET_COOKIE).iter().find_map(|h| {
        let (name, value) = h
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .split_once('=')?;
        (name == crate::auth::AUTH_COOKIE && !value.is_empty()).then(|| value.to_owned())
    })
}
