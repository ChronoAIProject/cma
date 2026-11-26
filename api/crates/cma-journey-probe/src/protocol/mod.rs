mod agui;
mod gateway;
mod http;
mod reads;
mod responses;
mod sse;

use std::{sync::Arc, time::Duration};

use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE},
    Client, RequestBuilder, Response,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    config::{secret, Config},
    model::{Check, Evidence, Intent, Job, Mode, Reason},
};

pub struct Protocol {
    config: Arc<Config>,
    client: Client,
}

#[derive(Default)]
pub struct Outcome {
    pub evidence: Evidence,
    pub settled: bool,
}

impl Protocol {
    pub fn new(config: Arc<Config>) -> Result<Self, &'static str> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| "cannot initialize HTTPS client")?;
        Ok(Self { config, client })
    }

    pub fn runner_sha(&self) -> &str {
        &self.config.runner_sha
    }

    pub fn intent(&self, job: Job, slot: i64) -> Result<Intent, &'static str> {
        let key = Intent::key(&self.config.domain(), job, slot);
        let marker = (job.mode == Mode::Deterministic).then(|| format!("CMA-CHECK-{key}"));
        let mut target = String::new();
        let mut body = String::new();
        if let Some(targets) = self.config.targets(job.mode) {
            let deterministic_prompt = marker
                .as_ref()
                .map(|marker| format!("Return this exact text: {marker}"));
            let prompt = deterministic_prompt
                .as_deref()
                .or(self.config.funded.as_ref().map(|v| v.prompt.as_str()))
                .ok_or("missing test prompt")?;
            let request = match job.check {
                Check::Agui => {
                    target.clone_from(&targets.agui_agent);
                    json!({"threadId":target,"messages":[{"role":"user","content":prompt}]})
                }
                Check::Responses => {
                    target.clone_from(&targets.responses_agent);
                    json!({"conversation":target,"input":prompt,"stream":true,"store":true})
                }
                Check::Gateway => {
                    target.clone_from(&targets.gateway.channel);
                    json!({"specversion":"1.0","id":key,"type":targets.gateway.event_type,"source":targets.gateway.source.as_str(),
                        "subject":key,"datacontenttype":"application/json","data":{"text":prompt}})
                }
                _ => return Err("invalid turn check"),
            };
            body = request.to_string();
        } else if job.mode != Mode::Read {
            return Err("missing turn target");
        }
        Ok(Intent {
            job,
            slot,
            key,
            target,
            request_digest: digest(&body),
            body,
            marker,
            provenance: self.config.provenance(),
        })
    }

    pub async fn first(&self, intent: &Intent, outcome: &mut Outcome) -> Result<(), Reason> {
        if intent.job.mode == Mode::Read {
            outcome.settled = true;
            return self.read(intent.job.check).await;
        }
        match intent.job.check {
            Check::Agui => self.agui_send(intent, outcome).await,
            Check::Responses => self.responses_send(intent, outcome).await,
            Check::Gateway => self.gateway_send(intent, outcome).await,
            _ => Err(Reason::Contract),
        }
    }

    pub async fn verify(&self, intent: &Intent, outcome: &mut Outcome) -> Result<(), Reason> {
        // A changed revision can remap a channel, Agent, origin or admission principal.
        if intent.provenance.configuration != self.config.revision {
            return Err(Reason::Contract);
        }
        match intent.job.check {
            Check::Agui => self.agui_verify(intent, outcome).await,
            Check::Responses => self.responses_verify(intent, outcome).await,
            Check::Gateway => self.gateway_verify(intent, outcome).await,
            _ => Err(Reason::Contract),
        }
    }

    fn owner(&self, request: RequestBuilder, gateway: bool) -> Result<RequestBuilder, Reason> {
        let file = if gateway {
            &self.config.gateway_bearer_file
        } else {
            &self.config.owner_bearer_file
        };
        let key = secret(file)?;
        let key = key.to_str().map_err(|_| Reason::CredentialDenied)?;
        let mut value = HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| Reason::CredentialDenied)?;
        value.set_sensitive(true);
        Ok(request.header(AUTHORIZATION, value))
    }

    fn cma_url(&self, path: &str) -> Url {
        let mut url = self.config.cma.clone();
        url.set_path(path);
        url
    }

    fn gateway_url(&self, path: &str) -> Url {
        let mut url = self.config.gateway.clone();
        let path = format!("{}{path}", url.path().trim_end_matches('/'));
        url.set_path(&path);
        url
    }

    pub(super) async fn get_owner(
        &self,
        url: Url,
        gateway: bool,
        accept: &'static str,
    ) -> Result<Response, Reason> {
        Self::send(self.owner(self.client.get(url).header(ACCEPT, accept), gateway)?).await
    }

    pub(super) async fn post_cma(
        &self,
        intent: &Intent,
        path: &str,
        sse: bool,
    ) -> Result<Response, Reason> {
        let accept = if sse {
            "text/event-stream"
        } else {
            "application/json"
        };
        let request = self
            .client
            .post(self.cma_url(path))
            .header(ACCEPT, accept)
            .header(CONTENT_TYPE, "application/json")
            .header("Idempotency-Key", &intent.key)
            .body(intent.body.clone());
        Self::send(self.owner(request, false)?)
            .await
            .map_err(|error| if sse { Reason::UnknownSend } else { error })
    }

    async fn send(request: RequestBuilder) -> Result<Response, Reason> {
        request.send().await.map_err(|_| Reason::Transport)
    }
}

pub(super) fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn check_answer(text: &str, intent: &Intent) -> Result<String, Reason> {
    if text.trim().is_empty()
        || text.len() > 32 * 1024
        || intent
            .marker
            .as_ref()
            .is_some_and(|marker| text.trim() != marker)
    {
        return Err(Reason::Contract);
    }
    Ok(digest(text))
}

fn submitted(response: &Response, outcome: &mut Outcome) -> Result<(), Reason> {
    outcome.evidence.http_status = Some(response.status().as_u16());
    let status = http::status(response);
    // The operation is definitely unsubmitted only for authentication refusal.
    if matches!(
        status,
        Err(Reason::CredentialDenied | Reason::CredentialExpired)
    ) {
        outcome.settled = true;
    }
    status
}

fn set_header_identity(headers: &HeaderMap, outcome: &mut Outcome) {
    outcome.evidence.command = http::header_id(headers, "x-cma-command-id");
    outcome.evidence.response = http::header_id(headers, "x-cma-response-id");
}

fn bind(value: &mut Option<String>, expected: &str) -> Result<(), Reason> {
    if value.as_deref().is_some_and(|known| known != expected) {
        return Err(Reason::Contract);
    }
    *value = Some(expected.to_owned());
    Ok(())
}

fn timestamp(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(|value| {
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).is_ok()
    })
}
