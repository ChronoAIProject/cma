use super::{
    operator_http::{self, Command},
    unresolved::{Binding, Request},
};
use std::time::Duration;

fn parse(args: &[&str]) -> Result<Command, &'static str> {
    let mut action = match args.first().copied() {
        Some("--replica-list-unresolved") => "list",
        Some("--replica-abandonment-audit") => "audit",
        Some("--replica-abandon-unknown") => "",
        _ => return Err("invalid replica command"),
    };
    let mut values = std::collections::BTreeMap::new();
    let mut at = 1;
    while at < args.len() {
        if matches!(args[at], "--dry-run" | "--apply") {
            if !action.is_empty() {
                return Err("choose one operator mode");
            }
            action = if args[at] == "--apply" {
                "apply"
            } else {
                "dry_run"
            };
            at += 1;
        } else {
            let value = args.get(at + 1).ok_or("missing operator argument")?;
            if values.insert(args[at], *value).is_some() {
                return Err("duplicate operator argument");
            }
            at += 2;
        }
    }
    let audit = if action == "audit" {
        Some(
            values
                .remove("--request-id")
                .ok_or("request-id required")?
                .to_owned(),
        )
    } else {
        None
    };
    let request = if matches!(action, "dry_run" | "apply") {
        let request = Request {
            binding: Binding {
                partition: values
                    .remove("--partition")
                    .ok_or("partition required")?
                    .into(),
                head_work: values
                    .remove("--head-work")
                    .ok_or("head-work required")?
                    .into(),
                head_version: values
                    .remove("--head-version")
                    .ok_or("head-version required")?
                    .parse()
                    .map_err(|_| "invalid head-version")?,
            },
            request_id: values
                .remove("--request-id")
                .ok_or("request-id required")?
                .into(),
            reason: values.remove("--reason").ok_or("reason required")?.into(),
            plan_digest: values.remove("--plan-digest").map(str::to_owned),
        };
        if action == "apply" && request.plan_digest.is_none() {
            return Err("plan-digest required for apply");
        }
        Some(request)
    } else {
        None
    };
    if action.is_empty() || !values.is_empty() {
        return Err("unknown or missing operator argument");
    }
    Ok(Command {
        action: action.into(),
        request,
        audit,
    })
}
pub fn valid_args(args: &[&str]) -> bool {
    parse(args).is_ok()
}
pub async fn run(args: &[&str]) -> Result<(), &'static str> {
    let command = parse(args)?;
    let config = crate::Config::from_env().map_err(|_| "operator configuration invalid")?;
    if !super::config::Config::from_env()?.enabled {
        return Err("operator command requires replica mode");
    }
    let body = serde_json::to_vec(&command).map_err(|_| "invalid operator body")?;
    let ticket = operator_http::ticket(
        &config.management.master_keys,
        &config.management.public_url,
        config.listen.port(),
        &body,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )
    .map_err(|_| "operator authentication unavailable")?;
    let host = if config.listen.is_ipv6() {
        "[::1]"
    } else {
        "127.0.0.1"
    };
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|_| "operator client unavailable")?
        .post(format!(
            "http://{host}:{}{}",
            config.listen.port(),
            operator_http::PATH
        ))
        .bearer_auth(ticket)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| "operator result uncertain; inspect the same request-id")?;
    let status = response.status();
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "operator result unavailable")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|_| "operator response invalid")?
    );
    if !status.is_success() {
        return Err(if value["code"] == "operator_outcome_uncertain" {
            "operator outcome uncertain; retrieve the audit using the same request-id"
        } else {
            "operator request refused"
        });
    }
    Ok(())
}
