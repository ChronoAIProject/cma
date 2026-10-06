//! One-use setup links exchange for a bounded HttpOnly browser capability.
use super::{connections::failure, connections_ui::*};
use crate::connections::{
    model::*,
    service::{effective_status, random_nonce},
    ConnectionService, Error,
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;
use subtle::ConstantTimeEq;
const COOKIE: &str = "__Host-cmaeg-connection";

pub(crate) fn router(service: ConnectionService) -> Router {
    Router::new()
        .route("/connect/start/{setup_ref}", get(start))
        .route("/connect", get(page))
        .route("/connect/submit", post(submit))
        .with_state(service)
}
async fn start(
    State(service): State<ConnectionService>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let grant = service.open_capability(&token, "start")?;
        let (mut record, _) = service.store.connection(&grant.record_id).await?;
        validate(&record, &grant, &record.setup_nonce)?;
        if let Some(nonce) = &record.browser_nonce {
            let (current, _) = browser_record(&service, &headers).await?;
            if current.id != record.id || current.browser_nonce.as_ref() != Some(nonce) {
                return Err(Error::NotFound);
            }
        } else {
            record.browser_nonce = Some(random_nonce());
        }
        let session =
            service.capability(&record, "browser", record.browser_nonce.as_deref().unwrap())?;
        let mut response = Redirect::to("/connect").into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&format!(
                "{COOKIE}={session}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
                (record.expires_at - now()).max(0)
            ))
            .map_err(|_| Error::Unavailable)?,
        );
        response.headers_mut().insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
        Ok(super::problems::no_store(response))
    }
    .await;
    result.unwrap_or_else(|_| expired())
}
async fn page(State(service): State<ConnectionService>, headers: HeaderMap) -> Response {
    match browser_record(&service, &headers).await {
        Ok((record, version)) => {
            let record = match service.refresh_management(record.clone(), version).await {
                Ok((next, _)) => next,
                Err(_) => record,
            };
            render_record(&service, &record, None).await
        }
        Err(_) => expired(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Submission {
    csrf_token: String,
    intent: String,
    bot_token: Option<String>,
    label: Option<String>,
    manager_connection_ref: Option<String>,
    suggested_name: Option<String>,
    suggested_username: Option<String>,
    request_management: Option<String>,
}
async fn submit(
    State(service): State<ConnectionService>,
    headers: HeaderMap,
    form: Result<Form<Submission>, axum::extract::rejection::FormRejection>,
) -> Response {
    let result = async {
        let (mut record, version) = browser_record(&service, &headers).await?;
        let mut origins = headers.get_all(header::ORIGIN).iter();
        if origins.next().and_then(|v| v.to_str().ok())
            != Some(service.management.public_url.trim_end_matches('/'))
            || origins.next().is_some()
        {
            return Err(Error::NotFound);
        }
        let Form(form) =
            form.map_err(|_| Error::Invalid("The setup form was invalid. Please try again."))?;
        let csrf = service.open_capability(&form.csrf_token, "csrf")?;
        validate(
            &record,
            &csrf,
            record.browser_nonce.as_deref().ok_or(Error::NotFound)?,
        )?;
        let requested_manager = match form.request_management.as_deref() {
            None => false,
            Some("true") => true,
            _ => return Err(Error::Invalid("Invalid capability selection.")),
        };
        // Link previews may open GETs, but only this authenticated, CSRF-checked
        // human POST claims the browser nonce durably.
        record.browser_claimed = true;
        let version = service
            .store
            .save_connection(&record, Some(version))
            .await?;
        let outcome = match form.intent.as_str() {
            "connect_existing" => {
                service
                    .connect_existing(
                        &record,
                        version,
                        form.bot_token
                            .ok_or(Error::Invalid("Enter the bot token."))?,
                        nonempty(form.label),
                        requested_manager,
                    )
                    .await
            }
            "create_managed" => {
                service
                    .create_managed(
                        &record,
                        version,
                        form.manager_connection_ref
                            .as_deref()
                            .ok_or(Error::Invalid("Select a creation bot."))?,
                        nonempty(form.suggested_name).or_else(|| record.suggested_name.clone()),
                        nonempty(form.suggested_username),
                        requested_manager,
                    )
                    .await
            }
            _ => return Err(Error::Invalid("Invalid setup action.")),
        };
        match outcome {
            Ok(()) => Ok(super::problems::no_store(
                Redirect::to("/connect").into_response(),
            )),
            Err(Error::Invalid(message)) => {
                Ok(render_record(&service, &record, Some(message.into())).await)
            }
            Err(e) => Err(e),
        }
    }
    .await;
    match result {
        Ok(response) => response,
        Err(Error::NotFound) => expired(),
        Err(error) => failure(error),
    }
}
fn nonempty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}
pub(crate) async fn browser_record(
    service: &ConnectionService,
    headers: &HeaderMap,
) -> Result<(Connection, u64), Error> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        let value = value.to_str().map_err(|_| Error::NotFound)?;
        for field in value.split(';') {
            let Some((name, token)) = field.trim().split_once('=') else {
                continue;
            };
            if name == COOKIE {
                if found.is_some() {
                    return Err(Error::NotFound);
                }
                found = Some(token);
            }
        }
    }
    let grant = service.open_capability(found.ok_or(Error::NotFound)?, "browser")?;
    let (mut record, version) = service.store.connection(&grant.record_id).await?;
    if grant.link_nonce.as_deref() != Some(&record.setup_nonce) {
        return Err(Error::NotFound);
    }
    if record.browser_claimed && record.browser_nonce.is_none() {
        return Err(Error::NotFound);
    }
    let expected = record.browser_nonce.as_deref().unwrap_or(&grant.nonce);
    validate(&record, &grant, expected)?;
    if record.browser_nonce.is_none() {
        record.browser_nonce = Some(grant.nonce);
    }
    Ok((record, version))
}
fn validate(record: &Connection, grant: &SetupCapability, nonce: &str) -> Result<(), Error> {
    if matches!(
        effective_status(record),
        Status::Cancelled | Status::Expired
    ) || record.expires_at <= now()
        || record.id != grant.record_id
        || record.expires_at != grant.expires_at
        || !bool::from(nonce.as_bytes().ct_eq(grant.nonce.as_bytes()))
    {
        return Err(Error::NotFound);
    }
    Ok(())
}
async fn render_record(
    service: &ConnectionService,
    record: &Connection,
    error: Option<String>,
) -> Response {
    let state=match effective_status(record) {
        Status::Draft=>{
            let managers=match service.manager(record).await {
                Ok((_,bot))=>vec![ManagerChoice {connection_ref:record.origin.event.record_id.to_string(),label:bot.username.unwrap_or(bot.first_name)}],
                Err(_)=>vec![],
            };
            let selected=if managers.is_empty() {ConnectionMethod::Existing} else {ConnectionMethod::Create};
            let csrf=match record.browser_nonce.as_deref().map(|nonce|service.capability(record,"csrf",nonce)) {Some(Ok(csrf))=>csrf,_=>return expired()};
            ConnectionSetupState::Choose(ConnectionChoices {form_action:"/connect/submit".into(),csrf_token:csrf,selected,managers,
                management:Some(ManagementChoice {instructions:"To enable this capability, select your bot in the official @BotFather Mini App and enable Bot Management Mode. The gateway verifies it with Telegram.".into(),enable_link:PageLink {label:"Open official @BotFather".into(),href:"https://t.me/BotFather".into()}}),
                registration_help:None,error,
            })
        },
        Status::AwaitingPlatform=>ConnectionSetupState::AwaitingPlatform {
            message:if record.material.is_some() {"Enable Bot Management Mode for your new bot in official @BotFather, then check the connection."} else {"Return to your Telegram conversation and use the Create a bot button. Telegram will ask you to confirm the new bot."}.into(),
            action:record.material.as_ref().map(|_|PageLink {label:"Open official @BotFather".into(),href:"https://t.me/BotFather".into()}),refresh_path:"/connect".into(),
        },
        Status::Ready=>ConnectionSetupState::Complete {connection_label:record.material.as_ref().map_or("Your bot",|m|m.name.as_str()).into(),action:None},
        _=>ConnectionSetupState::Expired {restart:None},
    };
    render(&ConnectionSetupPage {
        owner_label: format!(
            "{} {}",
            if record.organization {
                "Organization"
            } else {
                "Account"
            },
            record.owner
        ),
        platform_label: "Telegram".into(),
        state,
    })
    .unwrap_or_else(|_| failure(Error::Unavailable))
}
fn expired() -> Response {
    render(&ConnectionSetupPage {
        owner_label: "your account".into(),
        platform_label: "Telegram".into(),
        state: ConnectionSetupState::Expired { restart: None },
    })
    .unwrap_or_else(|_| failure(Error::Unavailable))
}
