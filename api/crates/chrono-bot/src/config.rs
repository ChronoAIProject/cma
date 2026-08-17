//! Configuration, read from the environment at startup.
//!
//! Every key here also appears in the deployment's ConfigMap or Secret. A key
//! read here but absent there is a divergence that will only show up in
//! production, so the two must be changed together.

use axum::http::HeaderValue;

use crate::auth::AuthMode;

/// Resolved configuration for the control plane.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to bind.
    pub bind: String,
    /// This service's public origin, used to build redirect URIs.
    pub public_url: String,
    /// Origins permitted to call this API from a browser.
    pub cors_origins: Vec<HeaderValue>,
    /// Base URL of the sandbox provider's lifecycle API.
    pub sandbox_base_url: String,
    /// API key for the sandbox provider.
    pub sandbox_api_key: String,
    /// Image each sandbox runs.
    pub sandbox_image: String,
    /// CPU each sandbox reserves, in Kubernetes quantity form.
    ///
    /// Configuration rather than a constant because it decides how many
    /// sessions a deployment can run at once: the namespace has a quota, and
    /// this figure divides into it. Sizing that from a rebuild means a session
    /// nobody can start until an image ships.
    pub sandbox_cpu: String,
    /// Memory each sandbox reserves, in Kubernetes quantity form.
    ///
    /// The tighter of the two in practice. The agent is what runs here, and
    /// an allowance it cannot work within is not a smaller session but a
    /// session that dies partway through somebody's work.
    pub sandbox_memory: String,
    /// What this deployment calls itself on the runtimes it creates.
    ///
    /// A sandbox provider is shared, so one key sees runtimes belonging to
    /// other control planes. This is what tells them apart.
    pub deployment_id: String,
    /// Whether runtimes carrying no deployment name are this one's to reclaim.
    ///
    /// True for the deployment that owns the provider, so runtimes created
    /// before names were recorded are still reclaimed once orphaned. Any
    /// additional control plane sharing the provider must set this false, or it
    /// will tear down sessions belonging to the first.
    pub adopts_untagged_runtimes: bool,
    /// Software every session carries, whoever started it.
    ///
    /// Part of what a session *is* on this platform rather than something a
    /// caller opted into, which is why an environment profile cannot remove an
    /// entry listed here.
    pub baseline_packages: Vec<String>,
    /// How the agent behaves in a session that names no profile.
    ///
    /// The deployment's answer to "start me a session" when nobody has said
    /// anything more. Configuration rather than a constant because which model
    /// is worth spending, and how freely an agent may act before asking, are
    /// decisions about this installation and not about the software.
    pub default_agent: layer2_core::AgentConfiguration,
    /// Services every session's credential may reach, whoever started it.
    ///
    /// A floor rather than a default. One credential serves the agent's model
    /// calls and the platform CLI alike, so a profile that excluded the model
    /// endpoint would leave the session unable to think rather than merely
    /// narrower.
    pub baseline_services: Vec<String>,
    /// Skills every session loads, whoever started it.
    ///
    /// Identified the way the registry identifies a skill: either its
    /// generated identifier or its unique name, both of which resolve.
    pub baseline_skills: Vec<String>,
    pub upstream_base_url: String,
    /// Model the agent selects.
    pub upstream_model: String,
    /// Request shape the upstream speaks.
    pub upstream_wire_api: String,
    /// Root of the identity platform a session's CLI acts against.
    pub platform_base_url: String,
    /// Connection string for the durable session store.
    pub mongodb_uri: String,
    /// Database holding session records and their history.
    pub mongodb_database: String,
    /// How long a recorded action is kept.
    ///
    /// Configuration rather than a constant because how far back a deployment
    /// must be able to account for itself is a decision about that deployment.
    /// It reaches the database as an index, so lowering it removes what is
    /// already stored rather than only affecting what is stored next.
    pub activity_retention: std::time::Duration,
    /// Bucket holding stored session context.
    ///
    /// Named explicitly rather than defaulted, and the only bucket this
    /// service ever addresses. It creates no bucket and holds no capability to
    /// create one; an absent name switches context storage off.
    pub context_bucket: String,
    /// This service's own machine identity on the identity platform, used to
    /// store session context.
    ///
    /// Its own identity rather than a credential borrowed from elsewhere. The
    /// sandbox provider and the identity platform are separate trust domains,
    /// and a key issued for one must never be spendable at the other.
    ///
    /// The identifier is not secret; the secret is, and the two are delivered
    /// separately for that reason.
    pub storage_client_id: String,
    /// Secret half of the machine identity above.
    pub storage_client_secret: String,
    /// How callers are authenticated.
    pub auth_mode: AuthMode,
}

impl Config {
    /// Read configuration from the environment.
    pub fn from_env() -> Result<Self, String> {
        let bind = var_or("CHRONO_SERVER_BIND", "0.0.0.0:8080");
        let public_url = var_or("CHRONO_SERVER_PUBLIC_URL", "http://localhost:8080");
        let sandbox_base_url = var_or(
            "CHRONO_SANDBOX_BASE_URL",
            "http://example-host-02.example-namespace.svc.cluster.local",
        );
        let sandbox_image = var_or("CHRONO_SANDBOX_IMAGE", "chrono-bot-runtime:dev");
        // What one session costs the namespace quota. Read as opaque strings
        // and handed to the provider unchanged: these are Kubernetes
        // quantities, and parsing them here would only add a way to be wrong
        // about a syntax this service does not own.
        let sandbox_cpu = var_or("CHRONO_SANDBOX_CPU", "1");
        let sandbox_memory = var_or("CHRONO_SANDBOX_MEMORY", "1Gi");
        // How the agent is told to continue a conversation rather than start
        // one. Here rather than baked into the runtime image so a change in
        // the agent's own flags is a configuration edit, not a rebuild.
        let deployment_id = var_or("CHRONO_DEPLOYMENT_ID", "chrono-bot");
        let adopts_untagged_runtimes =
            var_or("CHRONO_ADOPT_UNTAGGED_RUNTIMES", "true").trim() != "false";
        // Defaulted rather than required. These are what a session is expected
        // to have on this platform, and an environment that forgot to name them
        // must not silently produce sessions without them. A deployment that
        // genuinely mandates nothing sets the keys to a single space.
        let baseline_packages = list_or("CHRONO_BASELINE_PACKAGES", "nyxid");
        let baseline_skills = list_or("CHRONO_BASELINE_SKILLS", "nyxid,ornn-agent-manual-cli");
        // Named the way a person reads them; the identifiers the provider
        // wants are resolved when a credential is minted.
        let baseline_services = list_or("CHRONO_BASELINE_SERVICES", "chrono-llm,ornn-api");
        // What a session is when nobody chose anything. Each key is read on
        // its own and a value the software does not recognise is refused
        // rather than silently replaced: a deployment that meant "ask first"
        // and typed it wrong must be told, not quietly made autonomous.
        let default_agent = layer2_core::AgentConfiguration {
            kernel: layer2_core::KernelId::new(var_or("CHRONO_DEFAULT_AGENT_KERNEL", "codex")),
            mode: parse_enum("CHRONO_DEFAULT_AGENT_MODE", "terminal")?,
            model: Some(var_or("CHRONO_DEFAULT_AGENT_MODEL", "gpt-5.6-sol"))
                .filter(|m| !m.trim().is_empty()),
            reasoning_effort: parse_enum("CHRONO_DEFAULT_AGENT_EFFORT", "high")?,
            permission: parse_enum("CHRONO_DEFAULT_AGENT_PERMISSION", "guarded")?,
            credential: parse_enum("CHRONO_DEFAULT_AGENT_CREDENTIAL", "delegated")?,
            // The baseline skills are what every session carries anyway, so a
            // profile-less session carries exactly them and nothing else.
            skills: list_or("CHRONO_BASELINE_SKILLS", "nyxid,ornn-agent-manual-cli")
                .into_iter()
                .map(layer2_core::SkillId::new)
                .collect(),
        };
        let upstream_base_url = var_or("CHRONO_UPSTREAM_BASE_URL", "");
        let upstream_model = var_or("CHRONO_UPSTREAM_MODEL", "gpt-5.6-sol");
        let upstream_wire_api = var_or("CHRONO_UPSTREAM_WIRE_API", "responses");
        // Defaults to the same provider this service authenticates against, so
        // a session acts on the platform as the person who signed in.
        let platform_base_url = var_or("CHRONO_PLATFORM_BASE_URL", "");
        // Required: sessions are durable, and a service that starts without a
        // store would accept work it silently cannot keep.
        let mongodb_uri = require("CHRONO_MONGODB_URI")?;
        let mongodb_database = var_or("CHRONO_MONGODB_DATABASE", "chrono_bot");
        // Ninety days by default: long enough to investigate something nobody
        // noticed at the time, short enough that a collection holding one
        // document per request does not grow without limit. A deployment with
        // a longer obligation raises it deliberately.
        let activity_retention = days("CHRONO_ACTIVITY_RETENTION_DAYS", 90)?;
        // No default. The bucket is created and owned by whoever runs the
        // platform, and every object this service writes lands in it, so
        // guessing a name would mean writing somewhere nobody chose.
        let context_bucket = var_or("CHRONO_CONTEXT_BUCKET", "");
        let storage_client_id = var_or("CHRONO_STORAGE_CLIENT_ID", "");
        let storage_client_secret = var_or("CHRONO_STORAGE_CLIENT_SECRET", "");
        // The key is read from a file rather than an environment variable so it
        // can be delivered by a mounted secret and rotated without a restart of
        // the surrounding config, matching how neighbouring control planes do it.
        let sandbox_api_key = match std::env::var("CHRONO_SANDBOX_API_KEY_FILE")
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            Some(path) => std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read CHRONO_SANDBOX_API_KEY_FILE at {path}: {e}"))?
                .trim()
                .to_string(),
            None => require("CHRONO_SANDBOX_API_KEY")?,
        };

        let cors_origins = var_or("CHRONO_SERVER_CORS_ORIGIN", "http://localhost:5173")
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                HeaderValue::from_str(s).map_err(|e| {
                    format!("CHRONO_SERVER_CORS_ORIGIN entry {s:?} is not usable: {e}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Trusted mode has to be asked for by name. Defaulting to it would mean
        // a missing provider configuration silently disables authentication.
        let auth_mode = match var_or("CHRONO_AUTH_MODE", "provider").as_str() {
            "trusted" => AuthMode::Trusted {
                subject: var_or("CHRONO_TRUSTED_SUBJECT", "local-developer"),
            },
            "provider" => {
                let base_url = require("CHRONO_NYXID_BASE_URL")?;
                let client_id = require("CHRONO_NYXID_CLIENT_ID")?;
                let client_secret = require("CHRONO_NYXID_CLIENT_SECRET")?;
                AuthMode::Provider {
                    base_url,
                    client_id,
                    client_secret,
                    public_url: public_url.clone(),
                }
            }
            other => {
                return Err(format!(
                    "CHRONO_AUTH_MODE must be \"provider\" or \"trusted\", got {other:?}"
                ))
            }
        };

        Ok(Self {
            bind,
            public_url,
            cors_origins,
            sandbox_base_url,
            sandbox_api_key,
            sandbox_image,
            sandbox_cpu,
            sandbox_memory,
            default_agent,
            deployment_id,
            adopts_untagged_runtimes,
            baseline_packages,
            baseline_skills,
            baseline_services,
            upstream_base_url,
            upstream_model,
            upstream_wire_api,
            platform_base_url,
            mongodb_uri,
            mongodb_database,
            activity_retention,
            context_bucket,
            storage_client_id,
            storage_client_secret,
            auth_mode,
        })
    }

    /// Name of the configured auth mode, for logging.
    pub fn auth_mode_name(&self) -> &'static str {
        match self.auth_mode {
            AuthMode::Provider { .. } => "provider",
            AuthMode::Trusted { .. } => "trusted",
        }
    }
}

fn var_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn require(key: &str) -> Result<String, String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{key} is required and was not set"))
}

/// Read a number of days as a duration.
fn days(key: &str, default: u64) -> Result<std::time::Duration, String> {
    parse_days(key, &var_or(key, &default.to_string()))
}

/// Turn a written number of days into a duration.
///
/// A value that is not a positive whole number of days is refused rather than
/// silently replaced by the default: a retention set to `"ninety"` or to `0`
/// is somebody trying to say something, and quietly ignoring it would keep or
/// discard records against their intent.
fn parse_days(key: &str, raw: &str) -> Result<std::time::Duration, String> {
    let days: u64 = raw
        .trim()
        .parse()
        .map_err(|e| format!("{key} must be a whole number of days, got {raw:?}: {e}"))?;
    if days == 0 {
        return Err(format!("{key} must be at least one day, got {raw:?}"));
    }
    Ok(std::time::Duration::from_secs(days * 86_400))
}

/// Read a comma-separated list, dropping blank entries.
///
/// Blank entries are dropped rather than kept as empty strings: a trailing
/// comma is the most common way to write one of these by hand, and an empty
/// package name would reach the runtime as an install of nothing.
/// Read a value the core models as a closed set of names.
///
/// Refused rather than defaulted when unrecognised. These decide how freely an
/// agent acts and whose access it spends, and a typo silently becoming the
/// default is exactly the failure that would not be noticed until it mattered.
fn parse_enum<T: serde::de::DeserializeOwned>(key: &str, default: &str) -> Result<T, String> {
    let raw = var_or(key, default);
    let quoted = format!("\"{}\"", raw.trim());
    serde_json::from_str(&quoted).map_err(|_| format!("{key} does not name a value: {raw:?}"))
}

fn list_or(key: &str, default: &str) -> Vec<String> {
    var_or(key, default)
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// A resolved configuration for tests, with every field set to something
/// plausible.
///
/// Here rather than in a test module because more than one module needs it,
/// and a second copy is a second thing to update when a key is added — which
/// is exactly when a stale copy stops compiling and gets filled in blindly.
#[cfg(test)]
impl Config {
    pub fn sample() -> Self {
        Config {
            bind: "0.0.0.0:8080".into(),
            public_url: "http://x".into(),
            cors_origins: vec![],
            sandbox_base_url: "http://r".into(),
            sandbox_api_key: "k".into(),
            sandbox_image: "img".into(),
            sandbox_cpu: "1".into(),
            sandbox_memory: "1Gi".into(),
            default_agent: Default::default(),
            baseline_packages: vec![],
            baseline_skills: vec![],
            baseline_services: vec![],
            deployment_id: "chrono-bot".into(),
            adopts_untagged_runtimes: true,
            upstream_base_url: "http://u".into(),
            upstream_model: "m".into(),
            upstream_wire_api: "responses".into(),
            platform_base_url: "http://p".into(),
            mongodb_uri: "mongodb://h/db".into(),
            mongodb_database: "db".into(),
            activity_retention: std::time::Duration::from_secs(90 * 86_400),
            context_bucket: "bkt".into(),
            storage_client_id: "sa-id".into(),
            storage_client_secret: "sa-secret".into(),
            auth_mode: AuthMode::Trusted {
                subject: "s".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_value_falls_back_to_the_default() {
        // An env var set to the empty string is a common deployment mistake and
        // must behave as absent rather than producing an empty bind address.
        assert_eq!(var_or("CHRONO_TEST_ABSENT_KEY", "fallback"), "fallback");
    }

    #[test]
    fn require_names_the_missing_key() {
        let err = require("CHRONO_TEST_DEFINITELY_ABSENT").expect_err("must fail");
        assert!(err.contains("CHRONO_TEST_DEFINITELY_ABSENT"), "got {err}");
    }

    #[test]
    fn a_retention_falls_back_to_its_default_in_whole_days() {
        let d = days("CHRONO_TEST_ABSENT_RETENTION", 90).expect("the default is usable");
        assert_eq!(d.as_secs(), 90 * 86_400);
        assert_eq!(
            parse_days("CHRONO_ACTIVITY_RETENTION_DAYS", " 7 ")
                .expect("whitespace is not a value")
                .as_secs(),
            7 * 86_400
        );
    }

    #[test]
    fn a_retention_that_is_not_a_number_of_days_is_refused_rather_than_defaulted() {
        // Quietly ignoring it would keep records for a length of time nobody
        // chose, which is the failure mode this exists to prevent.
        for raw in ["ninety", "-1", "90d", ""] {
            let err = parse_days("CHRONO_ACTIVITY_RETENTION_DAYS", raw)
                .expect_err("{raw:?} is not a number of days");
            assert!(err.contains("CHRONO_ACTIVITY_RETENTION_DAYS"), "got {err}");
        }
    }

    #[test]
    fn a_retention_of_no_days_is_refused_rather_than_deleting_everything() {
        let err =
            parse_days("CHRONO_ACTIVITY_RETENTION_DAYS", "0").expect_err("zero is not a retention");
        assert!(err.contains("at least one day"), "got {err}");
    }

    #[test]
    fn auth_mode_names_match_the_variants() {
        let c = Config::sample();
        assert_eq!(c.auth_mode_name(), "trusted");

        let c = Config {
            auth_mode: AuthMode::Provider {
                base_url: "http://p".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://x".into(),
            },
            ..c
        };
        assert_eq!(c.auth_mode_name(), "provider");
    }
}
