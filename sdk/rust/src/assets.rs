//! Anonymous retrieval of the supplemental browser embed script.

use crate::{
    Client, Error, Result,
    http::{Request, receive},
};
use reqwest::{StatusCode, header};

/// Conditional retrieval options for the fixed browser embed script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbedScriptOptions {
    /// Entity tag previously returned by this deployment.
    pub if_none_match: Option<String>,
}

/// Exact browser script bytes or a validated cache hit; neither result executes code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedScriptResponse {
    /// Fresh browser script from the deployment's public asset path.
    Modified {
        /// Exact UTF-8 JavaScript source.
        script: String,
        /// Entity tag when provided.
        etag: Option<String>,
    },
    /// The supplied entity tag still identifies the script.
    NotModified {
        /// Current entity tag when provided.
        etag: Option<String>,
    },
}

impl Client {
    /// Read `/embed/v1.js` anonymously without executing it or following redirects.
    ///
    /// This is a supplemental browser asset, not an invented Layer1 API operation.
    pub async fn read_embed_script(
        &self,
        options: &EmbedScriptOptions,
    ) -> Result<EmbedScriptResponse> {
        let mut request = Request::get(&["embed", "v1.js"]);
        request.root = true;
        request.anonymous = true;
        request.statuses.push(StatusCode::NOT_MODIFIED);
        request.headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/javascript"),
        );
        if let Some(etag) = &options.if_none_match {
            if etag.trim().is_empty() {
                return Err(Error::validation("script entity tag must not be empty"));
            }
            request.headers.insert(
                header::IF_NONE_MATCH,
                header::HeaderValue::from_str(etag).map_err(|_| {
                    Error::validation("script entity tag must be a valid HTTP header")
                })?,
            );
        }
        let response = receive(&self.inner, request).await?;
        let etag = response
            .headers
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if response.status == StatusCode::NOT_MODIFIED {
            if options.if_none_match.is_none() {
                return Err(Error::protocol("unsolicited 304 script response"));
            }
            if !response.body.is_empty() {
                return Err(Error::protocol(
                    "304 script response unexpectedly carried a body",
                ));
            }
            return Ok(EmbedScriptResponse::NotModified { etag });
        }
        let kind = response
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if !kind.is_some_and(|kind| {
            kind.eq_ignore_ascii_case("application/javascript")
                || kind.eq_ignore_ascii_case("text/javascript")
        }) {
            return Err(Error::protocol(
                "embed script response did not carry JavaScript content",
            ));
        }
        let script = String::from_utf8(response.body)
            .map_err(|_| Error::protocol("embed script was not UTF-8"))?;
        Ok(EmbedScriptResponse::Modified { script, etag })
    }
}
