//! Bounded Graph transport with exact app/account/phone custody checks.
use crate::net::exchange::{read_bounded, HttpReply, TransportError};
use cmaeg_core::channel::{whatsapp::valid_meta_id, Source, WhatsappCredentials};
use futures_util::future::BoxFuture;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Method, Url,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

const RESPONSE_LIMIT: usize = 256 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Invalid,
    CustodyUnproved,
    Rejected,
    RateLimited,
    Unconfirmed,
}

pub trait Transport: Send + Sync {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>>;
}
struct HttpTransport(reqwest::Client);
impl Transport for HttpTransport {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(async move {
            let response = self
                .0
                .request(method, url)
                .headers(headers)
                .body(body)
                .timeout(timeout)
                .send()
                .await
                .map_err(|_| TransportError::Unavailable)?;
            let status = response.status();
            let headers = response.headers().clone();
            let body = read_bounded(response, RESPONSE_LIMIT).await;
            Ok(HttpReply {
                status,
                headers,
                body,
            })
        })
    }
}

#[derive(Clone)]
pub struct Client {
    transport: Arc<dyn Transport>,
    timeout: Duration,
}
impl Client {
    pub fn production() -> Result<Self, crate::ConfigError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| crate::ConfigError::InvalidManagement {
                detail: "cannot configure native WhatsApp transport".into(),
            })?;
        Ok(Self {
            transport: Arc::new(HttpTransport(http)),
            timeout: Duration::from_secs(8),
        })
    }
    pub fn with_transport(mut self, transport: Arc<dyn Transport>) -> Self {
        self.transport = transport;
        self
    }
    pub(super) fn endpoint(path: &str) -> Result<Url, Error> {
        Url::parse(&format!(
            "https://graph.facebook.com/{}/{path}",
            super::GRAPH_API_VERSION
        ))
        .map_err(|_| Error::Invalid)
    }
    pub(super) async fn call(
        &self,
        method: Method,
        url: Url,
        bearer: &str,
        body: Option<Value>,
    ) -> Result<Value, Error> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let body = body
            .map(|value| serde_json::to_vec(&value))
            .transpose()
            .map_err(|_| Error::Invalid)?
            .unwrap_or_default();
        self.call_bytes(method, url, bearer, headers, body).await
    }
    pub(super) async fn call_bytes(
        &self,
        method: Method,
        url: Url,
        bearer: &str,
        mut headers: HeaderMap,
        body: Vec<u8>,
    ) -> Result<Value, Error> {
        let mut auth =
            HeaderValue::from_str(&format!("Bearer {bearer}")).map_err(|_| Error::Invalid)?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        let reply = self
            .transport
            .exchange(method, url, headers, body, self.timeout)
            .await
            .map_err(|_| Error::Unconfirmed)?;
        let bytes = reply.body.map_err(|_| Error::Unconfirmed)?;
        if bytes.len() > RESPONSE_LIMIT {
            return Err(Error::Unconfirmed);
        }
        let parsed: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Unconfirmed)?;
        let code = parsed["error"]["code"].as_u64();
        if reply.status.as_u16() == 429 || matches!(code, Some(130429 | 80007)) {
            return Err(Error::RateLimited);
        }
        if !reply.status.is_success() || parsed.get("error").is_some() {
            return Err(if reply.status.is_client_error() {
                Error::Rejected
            } else {
                Error::Unconfirmed
            });
        }
        Ok(parsed)
    }

    /// Read-only authoring validation. Webhook payloads do not prove the app ID.
    /// Missing scopes, inaccessible account membership, or another app fail closed.
    pub async fn verify_custody(
        &self,
        source: &Source,
        credentials: &WhatsappCredentials,
    ) -> Result<(), Error> {
        let Source::Whatsapp {
            app_id,
            waba_id,
            phone_number_id,
            ..
        } = source
        else {
            return Err(Error::Invalid);
        };
        if [app_id, waba_id, phone_number_id]
            .iter()
            .any(|id| !valid_meta_id(id))
            || credentials.validate().is_err()
        {
            return Err(Error::Invalid);
        }
        let mut debug = Self::endpoint("debug_token")?;
        debug
            .query_pairs_mut()
            .append_pair("input_token", credentials.access_token.as_str());
        let app_token = format!("{app_id}|{}", credentials.app_secret.as_str());
        let answer = self.call(Method::GET, debug, &app_token, None).await?;
        let data = &answer["data"];
        let scopes = data["scopes"].as_array().ok_or(Error::CustodyUnproved)?;
        if data["is_valid"] != true
            || data["app_id"].as_str() != Some(app_id)
            || ![
                "whatsapp_business_management",
                "whatsapp_business_messaging",
            ]
            .iter()
            .all(|scope| scopes.iter().any(|value| value.as_str() == Some(scope)))
        {
            return Err(Error::CustodyUnproved);
        }
        let mut endpoint = Self::endpoint(&format!("{waba_id}/phone_numbers"))?;
        endpoint
            .query_pairs_mut()
            .append_pair("fields", "id")
            .append_pair("limit", "100");
        // Never follow provider-supplied pagination URLs with credentials. A bounded
        // first page that cannot establish this phone's membership fails closed.
        let account = self
            .call(
                Method::GET,
                endpoint,
                credentials.access_token.as_str(),
                None,
            )
            .await?;
        if !account["data"].as_array().is_some_and(|phones| {
            phones
                .iter()
                .any(|phone| phone["id"].as_str() == Some(phone_number_id))
        }) {
            return Err(Error::CustodyUnproved);
        }
        Ok(())
    }

    /// One actual native text send. Unknown acceptance is never automatically retried.
    pub async fn send_text(
        &self,
        credentials: &WhatsappCredentials,
        phone: &str,
        recipient: &str,
        parent: &str,
        text: &str,
    ) -> Result<String, Error> {
        if !valid_meta_id(phone)
            || !valid_meta_id(recipient)
            || !cmaeg_core::channel::whatsapp::valid_message_id(parent)
            || text.is_empty()
            || text.chars().count() > 4096
        {
            return Err(Error::Invalid);
        }
        let result = self.call(Method::POST, Self::endpoint(&format!("{phone}/messages"))?, credentials.access_token.as_str(), Some(json!({
            "messaging_product":"whatsapp", "recipient_type":"individual", "to":recipient,
            "context":{"message_id":parent}, "type":"text", "text":{"preview_url":false,"body":text}
        }))).await?;
        Self::message_identity(&result, recipient)
    }
    /// One bounded approved-template send, with no retry after uncertain acceptance.
    pub(crate) async fn send_template(
        &self,
        credentials: &WhatsappCredentials,
        template: super::templates::PreparedTemplate,
    ) -> Result<String, Error> {
        let mut client = self.clone();
        if template.media {
            // Meta resolves template media references; ordinary text templates
            // keep the native eight-second transport budget.
            client.timeout = Duration::from_secs(60);
        }
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let result = client
            .call_bytes(
                Method::POST,
                Self::endpoint(&format!("{}/messages", template.phone))?,
                credentials.access_token.as_str(),
                headers,
                template.body,
            )
            .await?;
        Self::message_identity(&result, &template.recipient)
    }
    pub(super) fn message_identity(result: &Value, recipient: &str) -> Result<String, Error> {
        if result["messaging_product"] != "whatsapp"
            || !result["contacts"].as_array().is_some_and(|contacts| {
                contacts.len() == 1 && contacts[0]["wa_id"].as_str() == Some(recipient)
            })
        {
            return Err(Error::Unconfirmed);
        }
        let messages = result["messages"]
            .as_array()
            .filter(|messages| messages.len() == 1)
            .ok_or(Error::Unconfirmed)?;
        messages[0]["id"]
            .as_str()
            .filter(|id| cmaeg_core::channel::whatsapp::valid_message_id(id))
            .map(str::to_owned)
            .ok_or(Error::Unconfirmed)
    }
}
