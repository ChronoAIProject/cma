//! Anonymous discovery and explicit unsupported-interface probes.

use crate::{Client, Error, Result, http::Request};
use reqwest::{Method, StatusCode};

/// Explicit unsupported-interface probes, without invented tasks or tool operations.
pub struct UnsupportedInterfaces<'a> {
    client: &'a Client,
}

impl UnsupportedInterfaces<'_> {
    /// Read the named MCP unavailability refusal without admitting any tool operation.
    pub async fn mcp(&self) -> Result<()> {
        self.client
            .refusal(Request::operation(
                Method::POST,
                &["mcp"],
                None::<&()>,
                StatusCode::OK,
            )?)
            .await
    }

    /// Read the named A2A unavailability refusal without admitting any task.
    pub async fn a2a(&self) -> Result<()> {
        self.client
            .refusal(Request::operation(
                Method::POST,
                &["a2a"],
                None::<&()>,
                StatusCode::OK,
            )?)
            .await
    }

    /// Probe anonymous agent-card discovery without sending the configured credential.
    pub async fn agent_card(&self) -> Result<()> {
        let mut request = Request::get(&[".well-known", "agent-card.json"]);
        request.root = true;
        request.anonymous = true;
        self.client.refusal(request).await
    }
}

impl Client {
    /// Read the deployment's anonymous OpenAPI document without sending a credential.
    pub async fn read_openapi(&self) -> Result<serde_json::Value> {
        let mut request = Request::get(&["openapi.json"]);
        request.root = true;
        request.anonymous = true;
        let document: serde_json::Value = self.request(request).await?;
        if !document["openapi"]
            .as_str()
            .is_some_and(|version| version.starts_with("3."))
            || !document["paths"].is_object()
        {
            return Err(Error::protocol(
                "discovery did not return an OpenAPI 3 document",
            ));
        }
        Ok(document)
    }

    /// Probe declared unsupported interfaces while preserving their named API refusals.
    pub fn unsupported(&self) -> UnsupportedInterfaces<'_> {
        UnsupportedInterfaces { client: self }
    }
}
