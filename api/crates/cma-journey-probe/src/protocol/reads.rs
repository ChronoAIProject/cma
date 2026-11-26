use reqwest::header::{ACCEPT, COOKIE};
use std::cell::RefCell;

use html5ever::tokenizer::{
    states::RawKind, BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer,
};
use url::Url;

use crate::{
    config::secret,
    model::{Check, Reason},
};

use super::{http, Protocol};

impl Protocol {
    pub(super) async fn read(&self, check: Check) -> Result<(), Reason> {
        match check {
            Check::Web => self.web().await,
            Check::Discovery => self.discovery().await,
            Check::Browser => self.browser().await,
            Check::Agent => self.agent().await,
            Check::AguiEvents => self.agui_reference().await,
            Check::GatewayEvents => self.gateway_reference().await,
            _ => Err(Reason::Contract),
        }
    }

    async fn web(&self) -> Result<(), Reason> {
        let response = Self::send(
            self.client
                .get(self.config.web.clone())
                .header(ACCEPT, "text/html"),
        )
        .await?;
        http::status(&response)?;
        if http::content_type(&response) != Some("text/html") {
            return Err(Reason::Contract);
        }
        let body = http::body(response, 256 * 1024).await?;
        let assets = assets(&self.config.web, &body)?;
        for (kind, url) in assets {
            let response = Self::send(self.client.get(url)).await?;
            http::status(&response)?;
            let valid = match kind {
                Asset::Script => matches!(
                    http::content_type(&response),
                    Some("application/javascript" | "text/javascript")
                ),
                Asset::Style => http::content_type(&response) == Some("text/css"),
            };
            if !valid {
                return Err(Reason::Contract);
            }
            let bytes = http::body(response, 8 * 1024 * 1024).await?;
            let code = std::str::from_utf8(&bytes)
                .map_err(|_| Reason::Contract)?
                .trim_start();
            if code.is_empty() || code.starts_with('<') {
                return Err(Reason::Contract);
            }
        }
        Ok(())
    }

    async fn discovery(&self) -> Result<(), Reason> {
        let url = self
            .config
            .issuer
            .join("/.well-known/openid-configuration")
            .map_err(|_| Reason::Contract)?;
        let value =
            http::json(Self::send(self.client.get(url).header(ACCEPT, "application/json")).await?)
                .await?;
        let issuer = value["issuer"]
            .as_str()
            .and_then(|v| Url::parse(v).ok())
            .ok_or(Reason::Contract)?;
        if issuer != self.config.issuer {
            return Err(Reason::Contract);
        }
        for field in ["authorization_endpoint", "token_endpoint", "jwks_uri"] {
            let url = value[field]
                .as_str()
                .and_then(|v| Url::parse(v).ok())
                .ok_or(Reason::Contract)?;
            if !same_origin(&issuer, &url) || url.path() == "/" {
                return Err(Reason::Contract);
            }
        }
        Ok(())
    }

    async fn browser(&self) -> Result<(), Reason> {
        let browser = self
            .config
            .browser
            .as_ref()
            .ok_or(Reason::CredentialMissing)?;
        let cookie = secret(&browser.cookie_file)?;
        let url = self.cma_url("/auth/session");
        let response = Self::send(self.client.get(url).header(COOKIE, cookie)).await?;
        let value = http::json(response).await?;
        if value["authenticated"] != true {
            return Err(Reason::CredentialDenied);
        }
        http::same(&value["subject"], &browser.subject)
    }

    async fn agent(&self) -> Result<(), Reason> {
        let reference = &self.config.reference;
        let value = self
            .response_json(&format!("/api/v1/agents/{}", reference.agent))
            .await?;
        http::same(&value["id"], &reference.agent)?;
        http::same(&value["sandbox"]["id"], &reference.sandbox)?;
        if !matches!(
            value["status"].as_str(),
            Some("ready" | "working" | "paused")
        ) {
            return Err(Reason::Contract);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Asset {
    Script,
    Style,
}

#[derive(Default)]
struct Document {
    roots: u32,
    template_depth: usize,
    base_href: bool,
    scripts: Vec<String>,
    styles: Vec<String>,
}

struct DocumentSink(RefCell<Document>);

impl TokenSink for DocumentSink {
    type Handle = ();

    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        use TokenSinkResult::{Continue, RawData};
        let Token::TagToken(tag) = token else {
            return Continue;
        };
        let mut doc = self.0.borrow_mut();
        let name: &str = &tag.name;
        if name == "template" {
            match tag.kind {
                TagKind::StartTag => doc.template_depth += 1,
                TagKind::EndTag => doc.template_depth = doc.template_depth.saturating_sub(1),
            }
        }
        if tag.kind != TagKind::StartTag {
            return Continue;
        }
        let attr = |name: &str| {
            tag.attrs
                .iter()
                .find(|a| a.name.local.as_ref() == name)
                .map(|a| a.value.as_ref())
        };
        if doc.template_depth == 0 {
            if attr("id") == Some("root") {
                doc.roots += 1;
            }
            match name {
                "base" => doc.base_href |= attr("href").is_some(),
                "script" if attr("type") == Some("module") => {
                    if let Some(src) = attr("src") {
                        doc.scripts.push(src.to_owned());
                    }
                }
                "link"
                    if attr("rel").is_some_and(|rel| {
                        rel.split_ascii_whitespace()
                            .any(|value| value.eq_ignore_ascii_case("stylesheet"))
                    }) =>
                {
                    if let Some(href) = attr("href") {
                        doc.styles.push(href.to_owned());
                    }
                }
                _ => {}
            }
        }
        match name {
            "script" => RawData(RawKind::ScriptData),
            "title" | "textarea" => RawData(RawKind::Rcdata),
            "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" => {
                RawData(RawKind::Rawtext)
            }
            "plaintext" => TokenSinkResult::Plaintext,
            _ => Continue,
        }
    }
}

fn assets(origin: &Url, bytes: &[u8]) -> Result<Vec<(Asset, Url)>, Reason> {
    let text = std::str::from_utf8(bytes).map_err(|_| Reason::Contract)?;
    let tokenizer = Tokenizer::new(
        DocumentSink(RefCell::new(Document::default())),
        Default::default(),
    );
    let input = BufferQueue::default();
    input.push_back(text.into());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let doc = tokenizer.sink.0.into_inner();
    if doc.roots != 1
        || doc.base_href
        || doc.scripts.is_empty()
        || doc.styles.is_empty()
        || doc.scripts.len() + doc.styles.len() > 8
    {
        return Err(Reason::Contract);
    }
    doc.scripts
        .into_iter()
        .map(|v| (Asset::Script, v))
        .chain(doc.styles.into_iter().map(|v| (Asset::Style, v)))
        .map(|(kind, reference)| {
            if reference.trim().is_empty() {
                return Err(Reason::Contract);
            }
            let url = origin.join(&reference).map_err(|_| Reason::Contract)?;
            if !same_origin(origin, &url) {
                return Err(Reason::Contract);
            }
            Ok((kind, url))
        })
        .collect()
}

fn same_origin(root: &Url, other: &Url) -> bool {
    root.origin() == other.origin()
        && other.username().is_empty()
        && other.password().is_none()
        && other.fragment().is_none()
}
