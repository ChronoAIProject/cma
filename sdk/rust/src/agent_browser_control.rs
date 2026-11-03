#![doc = "Exclusive, current-authority control of an already enabled Agent browser."]
use crate::{
    Client, Error, Result,
    http::{Request, receive_bounded},
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

string_enum! {
    #[doc = "Runtime-confirmed browser controller."]
    pub enum BrowserControlState { Agent => "agent", User => "user", Unknown => "unknown" }
}
string_enum! {
    #[doc = "An explicit idempotent browser control mutation."]
    pub enum BrowserControlCommand { Acquire => "acquire", Input => "input", Renew => "renew", Return => "return" }
}

#[doc = "Bounded browser input; never arbitrary script or desktop commands."]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserInput {
    #[doc = "A supported browser editing or navigation key without modifiers."]
    Key {
        #[doc = "Backspace, Tab, Enter, Escape, Home, End, arrows or Delete."]
        key: String,
    },
    #[doc = "A primary-button click in the 1280 by 720 viewport."]
    Click {
        #[doc = "Horizontal coordinate."]
        x: u16,
        #[doc = "Vertical coordinate."]
        y: u16,
    },
    #[doc = "Insert up to 1024 characters into the focused browser element."]
    Text {
        #[doc = "Text to insert; omitted from diagnostics and audit."]
        text: String,
    },
}
impl std::fmt::Debug for BrowserInput {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("BrowserInput([redacted])")
    }
}

#[doc = "The exact browser and client identity, optionally selecting a retained receipt."]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserControlScope {
    #[doc = "Current environment incarnation."]
    pub runtime_incarnation: String,
    #[doc = "Current browser context."]
    pub context_id: String,
    #[doc = "Unique controlling client identity."]
    pub client_id: String,
    #[doc = "Private random UUID sent in a header, never public audit or query URLs."]
    #[serde(default, skip_serializing)]
    pub client_key: String,
    #[doc = "Original operation to recover; absent for ordinary status/audit reads."]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}
impl std::fmt::Debug for BrowserControlScope {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("BrowserControlScope")
            .field("client_id", &self.client_id)
            .field("operation_id", &self.operation_id)
            .finish_non_exhaustive()
    }
}

#[doc = "Keep this exact body after an uncertain response; do not create another operation."]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserControlChange {
    #[doc = "Browser identity and required mutation operation id."]
    #[serde(flatten)]
    pub scope: BrowserControlScope,
    #[doc = "Requested mutation."]
    pub command: BrowserControlCommand,
    #[doc = "Last observed revision; every successful mutation advances it."]
    pub expected_revision: u64,
    #[doc = "Current lease authority, required except when acquiring."]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_token: Option<String>,
    #[doc = "Input payload for an input mutation only."]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<BrowserInput>,
}
impl std::fmt::Debug for BrowserControlChange {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("BrowserControlChange")
            .field("scope", &self.scope)
            .field("command", &self.command)
            .finish_non_exhaustive()
    }
}

#[doc = "Metadata-only audit entry retained in this browser incarnation."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct BrowserControlAudit {
    #[doc = "Monotonic control revision."]
    pub revision: u64,
    #[doc = "Acquired, input, returned, renewed, expired or unknown."]
    pub kind: String,
    #[doc = "Opaque authenticated actor and credential identity."]
    pub actor: String,
    #[doc = "Client responsible for the operation."]
    pub client_id: String,
    #[doc = "Original operation identity, absent for runtime expiry."]
    pub operation_id: Option<String>,
    #[doc = "Observed operation time."]
    pub at: String,
}

#[doc = "Correlated control receipt with current state and bounded audit."]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct BrowserControl {
    #[doc = "Agent being controlled."]
    pub agent_id: String,
    #[doc = "Runtime incarnation from environment discovery."]
    pub runtime_incarnation: String,
    #[doc = "The same browser context used for observation."]
    pub context_id: String,
    #[doc = "Current runtime-confirmed controller."]
    pub state: BrowserControlState,
    #[doc = "Latest control revision."]
    pub revision: u64,
    #[doc = "Current controlling client, when any."]
    pub client_id: Option<String>,
    #[doc = "Current lease token, returned only to the matching receipt owner."]
    pub lease_token: Option<String>,
    #[doc = "Lease expiry for display; runtime enforcement uses a monotonic clock."]
    pub expires_at: Option<String>,
    #[doc = "Fixed rolling lease duration: 15000 milliseconds."]
    pub lease_duration_ms: u32,
    #[doc = "Requested operation identity, absent for ordinary reads."]
    pub operation_id: Option<String>,
    #[doc = "Revision produced by that original operation."]
    pub operation_revision: Option<u64>,
    #[doc = "At most 128 entries without input contents or tokens."]
    pub audit: Vec<BrowserControlAudit>,
    #[doc = "First retained revision when older audit entries have been evicted."]
    pub audit_truncated_before: Option<u64>,
}
impl std::fmt::Debug for BrowserControl {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("BrowserControl")
            .field("state", &self.state)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

#[doc = "Public browser control APIs; each request rechecks authority."]
pub struct AgentBrowserControl<'client> {
    client: &'client Client,
}
impl Client {
    #[doc = "Control the existing browser through explicit runtime leases."]
    pub fn agent_browser_control(&self) -> AgentBrowserControl<'_> {
        AgentBrowserControl { client: self }
    }
}
impl AgentBrowserControl<'_> {
    #[doc = "Read current state/audit or query the original uncertain operation."]
    pub async fn read(&self, agent: &str, scope: &BrowserControlScope) -> Result<BrowserControl> {
        self.receive(agent, scope, None).await
    }
    #[doc = "Submit or exactly retry an explicit control operation."]
    pub async fn change(
        &self,
        agent: &str,
        input: &BrowserControlChange,
    ) -> Result<BrowserControl> {
        self.receive(agent, &input.scope, Some(input)).await
    }
    async fn receive(
        &self,
        agent: &str,
        scope: &BrowserControlScope,
        input: Option<&BrowserControlChange>,
    ) -> Result<BrowserControl> {
        let segments = ["agents", agent, "browser", "control"];
        let body = input
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| Error::protocol("Invalid control request"))?;
        let mut query = vec![
            ("runtime_incarnation", scope.runtime_incarnation.clone()),
            ("context_id", scope.context_id.clone()),
            ("client_id", scope.client_id.clone()),
        ];
        if let Some(operation) = &scope.operation_id {
            query.push(("operation_id", operation.clone()));
        }
        let mut request = Request::operation(
            if input.is_some() {
                Method::POST
            } else {
                Method::GET
            },
            &segments,
            body.as_ref(),
            StatusCode::OK,
        )?;
        if input.is_none() {
            request.query = &query;
        }
        request.headers.insert(
            "x-cma-browser-client",
            scope
                .client_key
                .parse()
                .map_err(|_| Error::protocol("Invalid browser client key"))?,
        );
        let bytes = receive_bounded(&self.client.inner, request, 65536).await?;
        let value: BrowserControl = serde_json::from_slice(&bytes.body)
            .map_err(|_| Error::protocol("Invalid control receipt"))?;
        if value.agent_id != agent
            || value.runtime_incarnation != scope.runtime_incarnation
            || value.context_id != scope.context_id
            || value.operation_id != scope.operation_id
            || value.lease_duration_ms != 15000
            || value.audit.len() > 128
            || (value.operation_id.is_some()
                && value
                    .operation_revision
                    .is_none_or(|revision| revision > value.revision))
            || input.is_some_and(|input| {
                input.expected_revision.checked_add(1) != value.operation_revision
            })
            || (value.state == BrowserControlState::User
                && (value.client_id.is_none() || value.expires_at.is_none()))
            || value.lease_token.as_ref().is_some_and(|token| {
                !uuid(token)
                    || value.state != BrowserControlState::User
                    || value.client_id.as_ref() != Some(&scope.client_id)
                    || scope.operation_id.is_none()
            })
            || value.audit.iter().any(|event| {
                event.revision > value.revision
                    || event.actor.len() > 128
                    || event.client_id.len() > 64
                    || event.at.len() > 32
                    || !matches!(
                        event.kind.as_str(),
                        "acquired" | "input" | "returned" | "renewed" | "expired" | "unknown"
                    )
                    || event
                        .operation_id
                        .as_ref()
                        .is_some_and(|operation| !uuid(operation))
            })
            || value
                .audit
                .windows(2)
                .any(|events| events[0].revision >= events[1].revision)
        {
            return Err(Error::protocol(
                "Browser control identity or bounds changed",
            ));
        }
        Ok(value)
    }
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
