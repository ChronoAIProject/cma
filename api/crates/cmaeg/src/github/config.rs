use serde::Deserialize;

use super::Error;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    apps: Vec<App>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct App {
    pub app_ref: String,
    pub app_id: u64,
    pub operator_owner: String,
    pub webhook_secret: String,
    pub attestations: Vec<Attestation>,
    #[serde(default)]
    pub routes: Vec<Route>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Route {
    pub channel_id: String,
    #[serde(default)]
    pub exclude_pull_request_head_prefixes: Vec<String>,
}

impl Route {
    pub(super) fn excludes(&self, event: &str, payload: &serde_json::Value) -> bool {
        event == "pull_request"
            && payload
                .pointer("/pull_request/head/ref")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|head| {
                    self.exclude_pull_request_head_prefixes
                        .iter()
                        .any(|prefix| head.starts_with(prefix))
                })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attestation {
    pub proof_ref: String,
    pub owner: String,
    pub installation_id: u64,
    pub account_id: u64,
    pub verified_until: i64,
    pub repository_ids: Vec<u64>,
}

impl Config {
    pub fn parse(json: &str) -> Result<Self, Error> {
        if json.len() > 1024 * 1024 {
            return Err(Error::Invalid);
        }
        let config: Self = serde_json::from_str(json).map_err(|_| Error::Invalid)?;
        let mut refs = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        if config.apps.is_empty() || config.apps.len() > 32 {
            return Err(Error::Invalid);
        }
        for app in &config.apps {
            if app.app_id == 0
                || app.operator_owner.is_empty()
                || app.operator_owner.len() > 512
                || !valid_ref(&app.app_ref)
                || app.webhook_secret.len() < 16
                || app.webhook_secret.len() > 4096
                || !refs.insert(&app.app_ref)
                || !ids.insert(app.app_id)
            {
                return Err(Error::Invalid);
            }
            let mut proofs = std::collections::BTreeSet::new();
            if app.routes.len() > 32 {
                return Err(Error::Invalid);
            }
            let mut routes = std::collections::BTreeSet::new();
            if app.routes.iter().any(|route| {
                cmaeg_core::ids::ChannelId::new(route.channel_id.clone()).is_err()
                    || !routes.insert(&route.channel_id)
                    || route.exclude_pull_request_head_prefixes.len() > 16
                    || route
                        .exclude_pull_request_head_prefixes
                        .iter()
                        .any(|prefix| prefix.is_empty() || prefix.len() > 128)
                    || route
                        .exclude_pull_request_head_prefixes
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != route.exclude_pull_request_head_prefixes.len()
            }) {
                return Err(Error::Invalid);
            }
            for proof in &app.attestations {
                if proof.owner.is_empty()
                    || proof.owner.len() > 512
                    || !valid_ref(&proof.proof_ref)
                    || proof.installation_id == 0
                    || proof.account_id == 0
                    || proof.repository_ids.contains(&0)
                    || proof.repository_ids.len() > 1000
                    || !proofs.insert(&proof.proof_ref)
                    || proof.verified_until > super::now() + 3600
                {
                    return Err(Error::Invalid);
                }
            }
        }
        Ok(config)
    }

    pub(super) fn app(&self, reference: &str) -> Option<&App> {
        self.apps.iter().find(|app| app.app_ref == reference)
    }

    pub(super) fn routes(&self, reference: &str) -> Option<&[Route]> {
        self.app(reference).map(|app| app.routes.as_slice())
    }

    pub(super) fn has_routes(&self) -> bool {
        self.apps.iter().any(|app| !app.routes.is_empty())
    }
}

pub(super) fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::Config;
    use serde_json::json;

    #[test]
    fn route_can_exclude_configured_pull_request_head_prefixes() {
        let config = Config::parse(
            &json!({
                "apps": [{
                    "app_ref": "pilot",
                    "app_id": 42,
                    "operator_owner": "platform-owner",
                    "webhook_secret": "fixture-webhook-secret",
                    "attestations": [],
                    "routes": [{
                        "channel_id": "cmaeg1.ch.loop-guard",
                        "exclude_pull_request_head_prefixes": ["automation/"]
                    }]
                }]
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(
            config.routes("pilot").unwrap()[0].exclude_pull_request_head_prefixes,
            ["automation/"],
        );
    }
}
