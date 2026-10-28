//! Management-only transport. The provisioning worker receives no message sender.
use super::{hash, JSON_VERSION, MAX_JSON_BYTES};
use crate::whatsapp::{Client, Error};
use cmaeg_core::channel::{whatsapp::valid_meta_id, Source, WhatsappCredentials};
use futures_util::future::BoxFuture;
use reqwest::{Method, Url};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc, time::Duration};

#[derive(Clone)]
pub(crate) struct Proof {
    pub id: String,
    pub published: bool,
    pub digest: String,
}
pub(crate) trait Manager: Send + Sync {
    fn verify<'a>(
        &'a self,
        source: &'a Source,
        credentials: &'a WhatsappCredentials,
    ) -> BoxFuture<'a, Result<(), Error>>;
    fn create<'a>(
        &'a self,
        source: &'a Source,
        credentials: &'a WhatsappCredentials,
        name: &'a str,
        schema: &'a Value,
    ) -> BoxFuture<'a, Result<String, Error>>;
    fn inspect<'a>(
        &'a self,
        source: &'a Source,
        credentials: &'a WhatsappCredentials,
        id: &'a str,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Proof, Error>>;
    fn publish<'a>(
        &'a self,
        credentials: &'a WhatsappCredentials,
        id: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>>;
    fn candidates<'a>(
        &'a self,
        source: &'a Source,
        credentials: &'a WhatsappCredentials,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>>;
}

pub(crate) trait AssetFetch: Send + Sync {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Vec<u8>, Error>>;
}
struct CdnFetch;
impl AssetFetch for CdnFetch {
    fn fetch<'a>(&'a self, raw: &'a str) -> BoxFuture<'a, Result<Vec<u8>, Error>> {
        Box::pin(async move {
            let url = asset_url(raw)?;
            tokio::time::timeout(Duration::from_secs(5), async {
                let addresses = crate::net::resolve::resolve(&url)
                    .await
                    .map_err(|_| Error::Unconfirmed)?;
                if !crate::net::resolve::all_public(&addresses) {
                    return Err(Error::CustodyUnproved);
                }
                let client = crate::net::resolve::pinned_client(
                    url.host_str().ok_or(Error::Invalid)?,
                    &addresses,
                    Duration::from_secs(2),
                    Duration::from_secs(4),
                )
                .map_err(|_| Error::Unconfirmed)?;
                // A fresh credentialless client; neither Graph bearer nor app secret can reach the CDN.
                let response = client
                    .get(url)
                    .send()
                    .await
                    .map_err(|_| Error::Unconfirmed)?;
                if !response.status().is_success() {
                    return Err(Error::Unconfirmed);
                }
                crate::net::exchange::read_bounded(response, MAX_JSON_BYTES)
                    .await
                    .map_err(|_| Error::Unconfirmed)
            })
            .await
            .map_err(|_| Error::Unconfirmed)?
        })
    }
}
fn asset_url(raw: &str) -> Result<Url, Error> {
    let url = crate::net::policy::target(raw).ok_or(Error::CustodyUnproved)?;
    if raw.len() > 8192
        || url.port().is_some_and(|p| p != 443)
        || !url
            .host_str()
            .is_some_and(|h| h.starts_with("scontent.") && h.ends_with(".fbcdn.net"))
    {
        return Err(Error::CustodyUnproved);
    }
    Ok(url)
}

#[derive(Clone)]
pub(crate) struct ManagementClient {
    graph: Client,
    fetch: Arc<dyn AssetFetch>,
}
impl ManagementClient {
    pub(crate) fn new(graph: Client) -> Self {
        Self {
            graph,
            fetch: Arc::new(CdnFetch),
        }
    }
    #[cfg(test)]
    pub(crate) fn with_fetch(mut self, fetch: Arc<dyn AssetFetch>) -> Self {
        self.fetch = fetch;
        self
    }
}

impl Manager for ManagementClient {
    fn verify<'a>(
        &'a self,
        source: &'a Source,
        credentials: &'a WhatsappCredentials,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { self.graph.verify_flow_custody(source, credentials).await })
    }
    fn create<'a>(
        &'a self,
        source: &'a Source,
        credentials: &'a WhatsappCredentials,
        name: &'a str,
        schema: &'a Value,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async move {
            let Source::Whatsapp {
                waba_id,
                forms_enabled: true,
                ..
            } = source
            else {
                return Err(Error::Invalid);
            };
            if !valid_meta_id(waba_id) || !valid_name(name) || schema["version"] != JSON_VERSION {
                return Err(Error::Invalid);
            }
            let encoded = serde_json::to_string(schema).map_err(|_| Error::Invalid)?;
            if encoded.len() > MAX_JSON_BYTES {
                return Err(Error::Invalid);
            }
            let answer = self.graph.call(Method::POST, Client::endpoint(&format!("{waba_id}/flows"))?, credentials.access_token.as_str(),
                Some(json!({"name":name,"categories":["OTHER"],"flow_json":encoded,"publish":false}))).await?;
            if answer["validation_errors"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
            {
                return Err(Error::Rejected);
            }
            if answer["success"] != true
                || !answer["validation_errors"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
            {
                return Err(Error::Unconfirmed);
            }
            answer["id"]
                .as_str()
                .filter(|id| valid_meta_id(id))
                .map(str::to_owned)
                .ok_or(Error::Unconfirmed)
        })
    }
    fn inspect<'a>(
        &'a self,
        source: &'a Source,
        credentials: &'a WhatsappCredentials,
        id: &'a str,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Proof, Error>> {
        Box::pin(async move {
            let Source::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                ..
            } = source
            else {
                return Err(Error::Invalid);
            };
            if !valid_meta_id(id) || !valid_name(name) {
                return Err(Error::Invalid);
            }
            let mut url = Client::endpoint(id)?;
            url.query_pairs_mut().append_pair("fields", &format!("id,name,status,validation_errors,json_version,endpoint_uri,whatsapp_business_account,application,health_status.phone_number({phone_number_id})"));
            let info = self
                .graph
                .call(Method::GET, url, credentials.access_token.as_str(), None)
                .await?;
            if info["id"] != id
                || info["name"] != name
                || info["json_version"] != JSON_VERSION
                || info["whatsapp_business_account"]["id"] != *waba_id
                || info["application"]["id"] != *app_id
                || !info["validation_errors"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
                || info
                    .get("endpoint_uri")
                    .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
                || !matches!(info["status"].as_str(), Some("DRAFT" | "PUBLISHED"))
            {
                return Err(Error::CustodyUnproved);
            }
            let published = info["status"] == "PUBLISHED";
            if published && info["health_status"]["can_send_message"] != "AVAILABLE" {
                return Err(Error::CustodyUnproved);
            }
            let assets = self
                .graph
                .call(
                    Method::GET,
                    Client::endpoint(&format!("{id}/assets"))?,
                    credentials.access_token.as_str(),
                    None,
                )
                .await?;
            let rows = assets["data"]
                .as_array()
                .filter(|v| v.len() == 1)
                .ok_or(Error::CustodyUnproved)?;
            if rows[0]["asset_type"] != "FLOW_JSON" || assets["paging"].get("next").is_some() {
                return Err(Error::CustodyUnproved);
            }
            let raw = rows[0]["download_url"]
                .as_str()
                .ok_or(Error::CustodyUnproved)?;
            asset_url(raw)?;
            let bytes = self.fetch.fetch(raw).await?;
            if bytes.len() > MAX_JSON_BYTES {
                return Err(Error::CustodyUnproved);
            }
            let schema: Value =
                super::unique_json::decode(&bytes).map_err(|_| Error::CustodyUnproved)?;
            Ok(Proof {
                id: id.into(),
                published,
                digest: hash(&schema),
            })
        })
    }
    fn publish<'a>(
        &'a self,
        credentials: &'a WhatsappCredentials,
        id: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !valid_meta_id(id) {
                return Err(Error::Invalid);
            }
            let answer = self
                .graph
                .call(
                    Method::POST,
                    Client::endpoint(&format!("{id}/publish"))?,
                    credentials.access_token.as_str(),
                    Some(json!({})),
                )
                .await?;
            if answer["success"] == true {
                Ok(())
            } else {
                Err(Error::Unconfirmed)
            }
        })
    }
    fn candidates<'a>(
        &'a self,
        source: &'a Source,
        credentials: &'a WhatsappCredentials,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async move {
            let Source::Whatsapp { waba_id, .. } = source else {
                return Err(Error::Invalid);
            };
            let mut cursor: Option<String> = None;
            let mut seen = BTreeSet::new();
            let mut found = Vec::new();
            for _ in 0..8 {
                let mut url = Client::endpoint(&format!("{waba_id}/flows"))?;
                url.query_pairs_mut()
                    .append_pair("fields", "id,name")
                    .append_pair("limit", "100");
                if let Some(after) = &cursor {
                    url.query_pairs_mut().append_pair("after", after);
                }
                let page = self
                    .graph
                    .call(Method::GET, url, credentials.access_token.as_str(), None)
                    .await?;
                let rows = page["data"]
                    .as_array()
                    .filter(|v| v.len() <= 100)
                    .ok_or(Error::CustodyUnproved)?;
                for row in rows {
                    let id = row["id"]
                        .as_str()
                        .filter(|id| valid_meta_id(id))
                        .ok_or(Error::CustodyUnproved)?;
                    if row["name"] == name {
                        found.push(id.to_owned());
                    }
                }
                if page["paging"].get("next").is_none() {
                    return Ok(found);
                }
                let after = page["paging"]["cursors"]["after"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 2048)
                    .ok_or(Error::CustodyUnproved)?;
                if !seen.insert(after.to_owned()) {
                    return Err(Error::CustodyUnproved);
                }
                cursor = Some(after.to_owned());
            }
            Err(Error::CustodyUnproved)
        })
    }
}
fn valid_name(name: &str) -> bool {
    name.starts_with("cmaeg_static_v1_")
        && name.len() == 80
        && name[16..].bytes().all(|b| b.is_ascii_hexdigit())
}

impl Client {
    pub(crate) async fn verify_flow_custody(
        &self,
        source: &Source,
        credentials: &WhatsappCredentials,
    ) -> Result<(), Error> {
        self.verify_custody(source, credentials).await?;
        let Source::Whatsapp {
            app_id,
            waba_id,
            forms_enabled: true,
            ..
        } = source
        else {
            return Err(Error::Invalid);
        };
        let mut url = Self::endpoint("debug_token")?;
        url.query_pairs_mut()
            .append_pair("input_token", credentials.access_token.as_str());
        let answer = self
            .call(
                Method::GET,
                url,
                &format!("{app_id}|{}", credentials.app_secret.as_str()),
                None,
            )
            .await?;
        let granular = answer["data"]["granular_scopes"]
            .as_array()
            .ok_or(Error::CustodyUnproved)?;
        if ![
            "whatsapp_business_management",
            "whatsapp_business_messaging",
        ]
        .iter()
        .all(|scope| {
            granular.iter().any(|grant| {
                grant["scope"] == *scope
                    && grant["target_ids"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| id == waba_id))
            })
        }) {
            return Err(Error::CustodyUnproved);
        }
        Ok(())
    }
}
