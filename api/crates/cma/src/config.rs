//! Configuration, read from the environment at startup.
//!
//! Every key here also appears in the deployment's ConfigMap or Secret. A key
//! read here but absent there is a divergence that will only show up in
//! production, so the two must be changed together.

use std::num::NonZeroU32;

use axum::http::HeaderValue;
use ipnet::IpNet;
use layer2_core::{RequiredCapability, TriggerRolloutPolicy};

use crate::auth::AuthMode;
use crate::store::AuthEncryptionKey;

const MONGODB_CUTOVER_STAGE: &str = "CHRONO_MONGODB_CUTOVER_STAGE";
const ADMISSION_RATE_REQUESTS_PER_MINUTE: u32 = 6_000;
const HEALTH_RATE_REQUESTS_PER_MINUTE: u32 = 120;
const MINIMUM_HEALTH_RATE_REQUESTS_PER_MINUTE: u32 = 30;
const DEFAULT_RATE_REQUESTS_PER_MINUTE: u32 = 600;
const CREATE_RATE_REQUESTS_PER_MINUTE: u32 = 10;
const INPUT_RATE_REQUESTS_PER_MINUTE: u32 = 120;
const DEFAULT_TRUSTED_PROXY_HOPS: usize = 1;
const MAX_TRUSTED_PROXY_HOPS: usize = 16;
const DEFAULT_TRIGGER_CHANGED_FILE_LIMIT: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerCleanupMode {
    DryRun,
    Execute,
}

impl TriggerCleanupMode {
    pub fn code(self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::Execute => "execute",
        }
    }
}

/// Deployment-owned Trigger rollout, scheduling, and retention controls.
#[derive(Debug, Clone)]
pub struct TriggerRuntimeConfig {
    pub rollout: TriggerRolloutPolicy,
    pub runtime_capabilities: Vec<RequiredCapability>,
    pub cleanup_mode: TriggerCleanupMode,
    pub changed_file_limit: u32,
    pub interval: std::time::Duration,
    pub page_size: u32,
    pub max_pages: usize,
    pub concurrency: usize,
}

impl TriggerRuntimeConfig {
    fn from_env() -> Result<Self, String> {
        let environment_enabled = strict_bool(
            "CHRONO_TRIGGER_ENABLED",
            &var_or("CHRONO_TRIGGER_ENABLED", "false"),
        )?;
        let orchestration_paused = strict_bool(
            "CHRONO_TRIGGER_ORCHESTRATION_PAUSED",
            &var_or("CHRONO_TRIGGER_ORCHESTRATION_PAUSED", "true"),
        )?;
        let capability_kill_switch = strict_bool(
            "CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH",
            &var_or("CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH", "true"),
        )?;
        let publisher_subject_ids = list_or("CHRONO_TRIGGER_PUBLISHER_SUBJECT_IDS", "");
        let repository_ids = parse_u64_list(
            "CHRONO_TRIGGER_REPOSITORY_IDS",
            &var_or("CHRONO_TRIGGER_REPOSITORY_IDS", ""),
        )?;
        let user_cohort_percent = parse_bounded_usize(
            "CHRONO_TRIGGER_USER_COHORT_PERCENT",
            &var_or("CHRONO_TRIGGER_USER_COHORT_PERCENT", "0"),
            0,
            100,
        )? as u8;
        let rollout = TriggerRolloutPolicy::new(
            environment_enabled,
            orchestration_paused,
            capability_kill_switch,
            publisher_subject_ids,
            repository_ids,
            user_cohort_percent,
        )
        .map_err(|error| error.to_string())?;
        let runtime_capabilities = parse_trigger_runtime_capabilities(
            "CHRONO_TRIGGER_RUNTIME_CAPABILITIES",
            &var_or("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", ""),
        )?;
        let cleanup_mode = parse_trigger_cleanup_mode(
            "CHRONO_TRIGGER_CLEANUP_MODE",
            &var_or("CHRONO_TRIGGER_CLEANUP_MODE", "dry_run"),
        )?;
        let changed_file_limit = parse_bounded_usize(
            "CHRONO_TRIGGER_CHANGED_FILE_LIMIT",
            &var_or(
                "CHRONO_TRIGGER_CHANGED_FILE_LIMIT",
                &DEFAULT_TRIGGER_CHANGED_FILE_LIMIT.to_string(),
            ),
            1,
            layer2_core::MAX_DIFF_SUMMARY_FILES,
        )? as u32;
        /*
         * Keep the scheduler's operation budget bounded independently of the
         * number of stored Launches. Values outside these ceilings fail
         * startup rather than turning a typo into an unbounded provider pass.
         */
        let interval_seconds = parse_bounded_usize(
            "CHRONO_TRIGGER_WORKER_INTERVAL_SECONDS",
            &var_or("CHRONO_TRIGGER_WORKER_INTERVAL_SECONDS", "5"),
            1,
            300,
        )?;
        let page_size = parse_bounded_usize(
            "CHRONO_TRIGGER_WORKER_PAGE_SIZE",
            &var_or("CHRONO_TRIGGER_WORKER_PAGE_SIZE", "50"),
            1,
            layer2_core::MAX_TRIGGER_PAGE_SIZE as usize,
        )? as u32;
        let max_pages = parse_bounded_usize(
            "CHRONO_TRIGGER_WORKER_MAX_PAGES",
            &var_or("CHRONO_TRIGGER_WORKER_MAX_PAGES", "20"),
            1,
            100,
        )?;
        let concurrency = parse_bounded_usize(
            "CHRONO_TRIGGER_WORKER_CONCURRENCY",
            &var_or("CHRONO_TRIGGER_WORKER_CONCURRENCY", "4"),
            1,
            32,
        )?;
        Ok(Self {
            rollout,
            runtime_capabilities,
            cleanup_mode,
            changed_file_limit,
            interval: std::time::Duration::from_secs(interval_seconds as u64),
            page_size,
            max_pages,
            concurrency,
        })
    }

    #[cfg(test)]
    fn sample() -> Self {
        Self {
            rollout: TriggerRolloutPolicy::allow_all(),
            runtime_capabilities: all_trigger_runtime_capabilities(),
            cleanup_mode: TriggerCleanupMode::DryRun,
            changed_file_limit: DEFAULT_TRIGGER_CHANGED_FILE_LIMIT as u32,
            interval: std::time::Duration::from_secs(5),
            page_size: 50,
            max_pages: 20,
            concurrency: 4,
        }
    }
}

fn parse_trigger_cleanup_mode(key: &str, raw: &str) -> Result<TriggerCleanupMode, String> {
    match raw {
        "dry_run" => Ok(TriggerCleanupMode::DryRun),
        "execute" => Ok(TriggerCleanupMode::Execute),
        other => Err(format!(
            "{key} must be \"dry_run\" or \"execute\", got {other:?}"
        )),
    }
}

#[cfg(test)]
fn all_trigger_runtime_capabilities() -> Vec<RequiredCapability> {
    vec![
        RequiredCapability::Conversation,
        RequiredCapability::DurableConversation,
        RequiredCapability::WorkspaceCheckpoint,
        RequiredCapability::RestrictedServiceGateway,
        RequiredCapability::CredentialRefresh,
        RequiredCapability::CredentialRevocation,
    ]
}

fn parse_trigger_runtime_capabilities(
    key: &str,
    raw: &str,
) -> Result<Vec<RequiredCapability>, String> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut parsed = Vec::new();
    for name in raw.split(',') {
        let capability = match name {
            "conversation" => RequiredCapability::Conversation,
            "durable_conversation" => RequiredCapability::DurableConversation,
            "workspace_checkpoint" => RequiredCapability::WorkspaceCheckpoint,
            "restricted_service_gateway" => RequiredCapability::RestrictedServiceGateway,
            "credential_refresh" => RequiredCapability::CredentialRefresh,
            "credential_revocation" => RequiredCapability::CredentialRevocation,
            _ => {
                return Err(format!(
                    "{key} contains unsupported capability {name:?}; use exact comma-separated canonical names"
                ));
            }
        };
        if parsed.contains(&capability) {
            return Err(format!("{key} contains duplicate capability {name:?}"));
        }
        parsed.push(capability);
    }
    Ok(parsed)
}

/// A deliberately incomplete control-plane startup used at a recovery boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MongoCutoverStage {
    /// Migrate only the legacy session namespaces, then remain quiescent.
    Legacy,
}

/// The only configuration a staged MongoDB cutover is allowed to observe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MongoCutoverStageConfig {
    /// The stage selected by the operator.
    pub stage: MongoCutoverStage,
    /// Address for the maintenance-only health listener.
    pub bind: String,
    /// Connection string for the one store the stage opens.
    pub mongodb_uri: String,
    /// Database whose legacy namespaces are migrated.
    pub mongodb_database: String,
}

impl MongoCutoverStageConfig {
    /// Read a staged startup before the normal control-plane configuration.
    pub fn from_env() -> Result<Option<Self>, String> {
        Self::from_lookup(strict_env)
    }

    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        let stage = lookup(MONGODB_CUTOVER_STAGE)?.unwrap_or_default();
        let stage = match stage.as_str() {
            "" => return Ok(None),
            "legacy" => MongoCutoverStage::Legacy,
            _ => {
                return Err(format!(
                    "{MONGODB_CUTOVER_STAGE} must be unset or exactly \"legacy\""
                ))
            }
        };

        let legacy = lookup("CHRONO_MONGODB_LEGACY_CUTOVER")?.unwrap_or_default();
        if legacy != "drained" {
            return Err(
                "CHRONO_MONGODB_CUTOVER_STAGE=legacy requires CHRONO_MONGODB_LEGACY_CUTOVER=drained after every legacy writer and its in-flight work have stopped"
                    .into(),
            );
        }
        let coordination = lookup("CHRONO_MONGODB_COORDINATION_CUTOVER")?.unwrap_or_default();
        if !coordination.is_empty() {
            return Err(
                "CHRONO_MONGODB_CUTOVER_STAGE=legacy requires CHRONO_MONGODB_COORDINATION_CUTOVER to be unset so coordination activation cannot begin"
                    .into(),
            );
        }

        let bind = lookup_or(&mut lookup, "CHRONO_SERVER_BIND", "0.0.0.0:8080")?;
        let mongodb_uri = require_from(&mut lookup, "CHRONO_MONGODB_URI")?;
        let mongodb_database = lookup_or(&mut lookup, "CHRONO_MONGODB_DATABASE", "chrono_bot")?;
        Ok(Some(Self {
            stage,
            bind,
            mongodb_uri,
            mongodb_database,
        }))
    }
}

/// Per-key request budgets enforced by this process.
#[derive(Debug, Clone, Copy)]
pub struct RateLimitConfig {
    /// Cheap per-network admission decisions made before authentication.
    pub admission_per_minute: NonZeroU32,
    /// Health requests, isolated from caller traffic so probes remain viable.
    pub health_per_minute: NonZeroU32,
    /// Requests per minute for routes without a stricter operation budget.
    pub default_per_minute: NonZeroU32,
    /// Session creations per minute.
    pub create_per_minute: NonZeroU32,
    /// Session input deliveries per minute.
    pub input_per_minute: NonZeroU32,
}

impl RateLimitConfig {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            admission_per_minute: positive_u32(
                "CHRONO_SERVER_RATE_ADMISSION_PER_MINUTE",
                ADMISSION_RATE_REQUESTS_PER_MINUTE,
            )?,
            health_per_minute: u32_at_least(
                "CHRONO_SERVER_RATE_HEALTH_PER_MINUTE",
                HEALTH_RATE_REQUESTS_PER_MINUTE,
                MINIMUM_HEALTH_RATE_REQUESTS_PER_MINUTE,
            )?,
            default_per_minute: positive_u32(
                "CHRONO_SERVER_RATE_DEFAULT_PER_MINUTE",
                DEFAULT_RATE_REQUESTS_PER_MINUTE,
            )?,
            create_per_minute: positive_u32(
                "CHRONO_SERVER_RATE_CREATE_PER_MINUTE",
                CREATE_RATE_REQUESTS_PER_MINUTE,
            )?,
            input_per_minute: positive_u32(
                "CHRONO_SERVER_RATE_INPUT_PER_MINUTE",
                INPUT_RATE_REQUESTS_PER_MINUTE,
            )?,
        })
    }

    #[cfg(test)]
    fn sample() -> Self {
        Self {
            admission_per_minute: NonZeroU32::new(ADMISSION_RATE_REQUESTS_PER_MINUTE).unwrap(),
            health_per_minute: NonZeroU32::new(HEALTH_RATE_REQUESTS_PER_MINUTE).unwrap(),
            default_per_minute: NonZeroU32::new(DEFAULT_RATE_REQUESTS_PER_MINUTE).unwrap(),
            create_per_minute: NonZeroU32::new(CREATE_RATE_REQUESTS_PER_MINUTE).unwrap(),
            input_per_minute: NonZeroU32::new(INPUT_RATE_REQUESTS_PER_MINUTE).unwrap(),
        }
    }
}

/// Which TCP peers may supply the load balancer's client-address chain.
#[derive(Debug, Clone)]
pub struct TrustedProxyConfig {
    /// Networks from which a forwarding header is accepted.
    pub cidrs: Vec<IpNet>,
    /// Tokens to skip from the right before selecting the client address.
    pub forwarded_hops: usize,
}

impl TrustedProxyConfig {
    fn from_env() -> Result<Self, String> {
        let cidrs = parse_ip_networks(
            "CHRONO_SERVER_TRUSTED_PROXY_CIDRS",
            &var_or("CHRONO_SERVER_TRUSTED_PROXY_CIDRS", ""),
        )?;
        let forwarded_hops = parse_bounded_usize(
            "CHRONO_SERVER_TRUSTED_PROXY_HOPS",
            &var_or(
                "CHRONO_SERVER_TRUSTED_PROXY_HOPS",
                &DEFAULT_TRUSTED_PROXY_HOPS.to_string(),
            ),
            1,
            MAX_TRUSTED_PROXY_HOPS,
        )?;
        Ok(Self {
            cidrs,
            forwarded_hops,
        })
    }

    #[cfg(test)]
    pub(crate) fn sample() -> Self {
        Self {
            cidrs: Vec::new(),
            forwarded_hops: DEFAULT_TRUSTED_PROXY_HOPS,
        }
    }
}

/// Resolved configuration for the control plane.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to bind.
    pub bind: String,
    /// This service's public origin, used to build redirect URIs.
    pub public_url: String,
    /// Origins permitted to call this API from a browser.
    pub cors_origins: Vec<HeaderValue>,
    /// Per-key request budgets enforced by this process.
    pub rate_limits: RateLimitConfig,
    /// Peers whose load-balancer forwarding chain may identify a client.
    pub trusted_proxies: TrustedProxyConfig,
    /// Base URL of the sandbox provider's lifecycle API.
    pub sandbox_base_url: String,
    /// API key for the sandbox provider.
    pub sandbox_api_key: String,
    /// Image each sandbox runs.
    pub sandbox_image: String,
    /// CPU a sandbox reserves when its session names no size, in Kubernetes quantity form.
    ///
    /// Configuration rather than a constant because it decides how many
    /// sessions a deployment can run at once: the namespace has a quota, and
    /// this figure divides into it. Sizing that from a rebuild means a session
    /// nobody can start until an image ships.
    pub sandbox_cpu: String,
    /// Memory a sandbox reserves when its session names no size, in Kubernetes quantity form.
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
    /// Where a sandbox forwards the second kernel's model calls.
    ///
    /// Falls back to the shared upstream when a deployment names none, which is
    /// the ordinary case for a gateway that serves both request shapes at one
    /// address.
    pub claude_upstream_base_url: String,
    /// Model the second kernel selects when a session names none.
    pub claude_upstream_model: String,
    /// Where a sandbox forwards the third kernel's model calls.
    ///
    /// Falls back to the shared upstream when a deployment names none.
    pub dsh_upstream_base_url: String,
    /// Model the third kernel selects when a session names none.
    pub dsh_upstream_model: String,
    /// Root of the identity platform a session's CLI acts against.
    pub platform_base_url: String,
    /// Connection string for the durable session store.
    pub mongodb_uri: String,
    /// Database holding session records and their history.
    pub mongodb_database: String,
    /// Explicit acknowledgement for the one-time legacy namespace cutover.
    ///
    /// This does not discover or stop old writers. It is accepted only when an
    /// operator has already stopped every pre-cutover control plane and waited
    /// for its MongoDB operations to drain.
    pub mongodb_legacy_cutover: bool,
    /// Explicit acknowledgement that every pre-coordination process and its
    /// in-flight provider operations have permanently drained.
    pub mongodb_coordination_cutover: bool,
    /// Dedicated key for recoverable authentication secrets stored in MongoDB.
    pub auth_encryption_key: AuthEncryptionKey,
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
    /// Bucket holding Trigger repository baselines and workspace checkpoints.
    ///
    /// Separate configuration from conversation context even when a deployment
    /// deliberately selects the same owned bucket. Repository artifacts have
    /// their own retention and deletion policy, and must never inherit the
    /// transcript lifecycle merely because both use platform object storage.
    pub trigger_workspace_bucket: String,
    /// GitHub Trigger rollout and bounded worker controls.
    pub trigger: TriggerRuntimeConfig,
    /// Current Ed25519 signer for short-lived runtime policies, when enabled.
    pub github_capability_signing: Option<GithubCapabilitySigningConfig>,
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

/// Secret signing material and public-key rotation inputs.
#[derive(Clone)]
pub struct GithubCapabilitySigningConfig {
    pub key_id: String,
    pub private_key_pkcs8_base64: String,
    pub previous_verification_keys_json: String,
}

impl std::fmt::Debug for GithubCapabilitySigningConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GithubCapabilitySigningConfig")
            .field("key_id", &self.key_id)
            .field("private_key_pkcs8_base64", &"<redacted>")
            .field(
                "previous_verification_keys_json",
                &self.previous_verification_keys_json,
            )
            .finish()
    }
}

/// Services every session's credential may reach when a deployment names none.
///
/// Named the way a person reads them; the identifiers the provider wants are
/// resolved when a credential is minted.
///
/// `ornn` and `ornn-api` are two services, not one name spelled twice.
/// `ornn-api` answers the skill registry this service itself searches to show
/// somebody what they may pick; `ornn` is the skill runtime the agent calls
/// once it has picked, and it is the one the baseline skills tell it to reach
/// for. A baseline naming only the registry mints a credential the provider
/// refuses on every call the agent actually makes.
///
/// A constant rather than a literal at the use site so the default is a thing
/// a test can hold, instead of a string a test would have to repeat — which
/// would pin the test to itself and nothing else.
const BASELINE_SERVICES: &str = "chrono-llm,ornn-api,ornn";

const DEFAULT_SANDBOX_IMAGE: &str = "cma-runtime:dev";

/// Deployment identity used only when a new installation omits the key.
///
/// Existing installations set this explicitly because it is persisted into
/// credential names and sandbox metadata; a rebrand must not change that
/// state identity underneath them.
const DEFAULT_DEPLOYMENT_ID: &str = "cma";

impl Config {
    /// Read configuration from the environment.
    pub fn from_env() -> Result<Self, String> {
        let bind = var_or("CHRONO_SERVER_BIND", "0.0.0.0:8080");
        let public_url = var_or("CHRONO_SERVER_PUBLIC_URL", "http://localhost:8080");
        let rate_limits = RateLimitConfig::from_env()?;
        let trusted_proxies = TrustedProxyConfig::from_env()?;
        let sandbox_base_url = var_or(
            "CHRONO_SANDBOX_BASE_URL",
            "http://opensandbox-server.opensandbox-system.svc.cluster.local",
        );
        let sandbox_image = var_or("CHRONO_SANDBOX_IMAGE", DEFAULT_SANDBOX_IMAGE);
        // What a sandbox costs the namespace quota when its session names no
        // size. Read as opaque strings and handed to the provider unchanged:
        // these are Kubernetes quantities, and parsing them here would only
        // add a way to be wrong about a syntax this service does not own.
        let sandbox_cpu = var_or("CHRONO_SANDBOX_CPU", "1");
        let sandbox_memory = var_or("CHRONO_SANDBOX_MEMORY", "1Gi");
        // How the agent is told to continue a conversation rather than start
        // one. Here rather than baked into the runtime image so a change in
        // the agent's own flags is a configuration edit, not a rebuild.
        let deployment_id = var_or("CHRONO_DEPLOYMENT_ID", DEFAULT_DEPLOYMENT_ID);
        let adopts_untagged_runtimes =
            var_or("CHRONO_ADOPT_UNTAGGED_RUNTIMES", "true").trim() != "false";
        // Defaulted rather than required. These are what a session is expected
        // to have on this platform, and an environment that forgot to name them
        // must not silently produce sessions without them. A deployment that
        // genuinely mandates nothing sets the keys to a single space.
        let baseline_packages = list_or("CHRONO_BASELINE_PACKAGES", "nyxid");
        let baseline_skills = list_or("CHRONO_BASELINE_SKILLS", "nyxid,ornn-agent-manual-cli");
        let baseline_services = list_or("CHRONO_BASELINE_SERVICES", BASELINE_SERVICES);
        // What a session is when nobody chose anything. Each key is read on
        // its own and a value the software does not recognise is refused
        // rather than silently replaced: a deployment that meant "ask first"
        // and typed it wrong must be told, not quietly made autonomous.
        let default_agent = layer2_core::AgentConfiguration {
            kernel: layer2_core::KernelId::new(var_or("CHRONO_DEFAULT_AGENT_KERNEL", "codex")),
            // Conversation, not terminal. The conversation plane is the
            // surface this product is for: it is where approvals are answered,
            // where tool calls read as structure, and where a failure can be
            // explained rather than printed. A terminal is the escape hatch.
            //
            // Only the deployment default moves. `SessionMode`'s serde default
            // stays Terminal deliberately — it is what every record written
            // before the field existed means, and reading those back as
            // conversations would silently change what they are.
            mode: parse_enum("CHRONO_DEFAULT_AGENT_MODE", "conversation")?,
            model: Some(var_or("CHRONO_DEFAULT_AGENT_MODEL", "gpt-5.6-sol"))
                .filter(|m| !m.trim().is_empty()),
            reasoning_effort: parse_enum("CHRONO_DEFAULT_AGENT_EFFORT", "high")?,
            permission: parse_enum("CHRONO_DEFAULT_AGENT_PERMISSION", "guarded")?,
            credential: parse_enum("CHRONO_DEFAULT_AGENT_CREDENTIAL", "delegated")?,
            // No key for this, deliberately. `None` already means
            // CHRONO_SANDBOX_CPU / CHRONO_SANDBOX_MEMORY, so a
            // CHRONO_DEFAULT_AGENT_SIZE would be a second way to say the same
            // thing that could disagree with the first.
            size: None,
            record_tool_bodies: false,
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
        // The second kernel's upstream. Its own keys because kernels do not
        // share a model and need not share an endpoint: one speaks a
        // completions-shaped API and the other a messages-shaped one, and a
        // deployment whose gateway serves both at one address simply leaves the
        // base URL unset and inherits the first.
        let claude_upstream_base_url = match var_or("CHRONO_CLAUDE_UPSTREAM_BASE_URL", "") {
            empty if empty.is_empty() => upstream_base_url.clone(),
            named => named,
        };
        let claude_upstream_model = var_or("CHRONO_CLAUDE_UPSTREAM_MODEL", "claude-opus-5");
        // The third kernel's, on the same terms and for the same reason.
        let dsh_upstream_base_url = match var_or("CHRONO_DSH_UPSTREAM_BASE_URL", "") {
            empty if empty.is_empty() => upstream_base_url.clone(),
            named => named,
        };
        let dsh_upstream_model = var_or("CHRONO_DSH_UPSTREAM_MODEL", "deepseek-v4-flash");
        // Defaults to the same provider this service authenticates against, so
        // a session acts on the platform as the person who signed in.
        let platform_base_url = var_or("CHRONO_PLATFORM_BASE_URL", "");
        // Required: sessions are durable, and a service that starts without a
        // store would accept work it silently cannot keep.
        let mongodb_uri = require("CHRONO_MONGODB_URI")?;
        let mongodb_database = var_or("CHRONO_MONGODB_DATABASE", "chrono_bot");
        let mongodb_legacy_cutover = parse_cutover_drain(
            "CHRONO_MONGODB_LEGACY_CUTOVER",
            &var_or("CHRONO_MONGODB_LEGACY_CUTOVER", ""),
        )?;
        let mongodb_coordination_cutover = parse_cutover_drain(
            "CHRONO_MONGODB_COORDINATION_CUTOVER",
            &var_or("CHRONO_MONGODB_COORDINATION_CUTOVER", ""),
        )?;
        let auth_encryption_key =
            AuthEncryptionKey::from_base64(&require("CHRONO_AUTH_ENCRYPTION_KEY")?)?;
        // Ninety days by default: long enough to investigate something nobody
        // noticed at the time, short enough that a collection holding one
        // document per request does not grow without limit. A deployment with
        // a longer obligation raises it deliberately.
        let activity_retention = days("CHRONO_ACTIVITY_RETENTION_DAYS", 90)?;
        // No default. The bucket is created and owned by whoever runs the
        // platform, and every object this service writes lands in it, so
        // guessing a name would mean writing somewhere nobody chose.
        let context_bucket = var_or("CHRONO_CONTEXT_BUCKET", "");
        let trigger_workspace_bucket = var_or("CHRONO_TRIGGER_WORKSPACE_BUCKET", "");
        let trigger = TriggerRuntimeConfig::from_env()?;
        let capability_key_id = var_or("CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_ID", "");
        let capability_private_key = var_or("CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_PKCS8", "");
        let previous_verification_keys_json =
            var_or("CHRONO_GITHUB_CAPABILITY_PREVIOUS_VERIFY_KEYS", "{}");
        let github_capability_signing = match (
            capability_key_id.is_empty(),
            capability_private_key.is_empty(),
        ) {
            (true, true) => None,
            (false, false) => Some(GithubCapabilitySigningConfig {
                key_id: capability_key_id,
                private_key_pkcs8_base64: capability_private_key,
                previous_verification_keys_json,
            }),
            _ => {
                return Err("CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_ID and \
                     CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_PKCS8 must be set together"
                    .into())
            }
        };
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
            rate_limits,
            trusted_proxies,
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
            claude_upstream_base_url,
            claude_upstream_model,
            dsh_upstream_base_url,
            dsh_upstream_model,
            platform_base_url,
            mongodb_uri,
            mongodb_database,
            mongodb_legacy_cutover,
            mongodb_coordination_cutover,
            auth_encryption_key,
            activity_retention,
            context_bucket,
            trigger_workspace_bucket,
            trigger,
            github_capability_signing,
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

fn strict_env(key: &str) -> Result<Option<String>, String> {
    match std::env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!(
            "{key} is not valid UTF-8 and cannot be interpreted"
        )),
    }
}

fn lookup_or(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
    default: &str,
) -> Result<String, String> {
    Ok(lookup(key)?
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_owned()))
}

fn require_from(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
) -> Result<String, String> {
    lookup(key)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required and was not set"))
}

fn require(key: &str) -> Result<String, String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{key} is required and was not set"))
}

fn strict_bool(key: &str, raw: &str) -> Result<bool, String> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!(
            "{key} must be exactly \"true\" or \"false\", got {raw:?}"
        )),
    }
}

fn parse_u64_list(key: &str, raw: &str) -> Result<Vec<u64>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    raw.split(',')
        .map(|part| {
            let part = part.trim();
            let value = part.parse::<u64>().map_err(|error| {
                format!("{key} contains invalid numeric identity {part:?}: {error}")
            })?;
            if value == 0 {
                return Err(format!("{key} contains invalid numeric identity 0"));
            }
            Ok(value)
        })
        .collect()
}

fn positive_u32(key: &str, default: u32) -> Result<NonZeroU32, String> {
    parse_positive_u32(key, &var_or(key, &default.to_string()))
}

fn parse_positive_u32(key: &str, raw: &str) -> Result<NonZeroU32, String> {
    let value: u32 = raw
        .trim()
        .parse()
        .map_err(|e| format!("{key} must be a positive whole number, got {raw:?}: {e}"))?;
    NonZeroU32::new(value).ok_or_else(|| format!("{key} must be greater than zero, got {raw:?}"))
}

fn u32_at_least(key: &str, default: u32, minimum: u32) -> Result<NonZeroU32, String> {
    parse_u32_at_least(key, &var_or(key, &default.to_string()), minimum)
}

fn parse_u32_at_least(key: &str, raw: &str, minimum: u32) -> Result<NonZeroU32, String> {
    let value = parse_positive_u32(key, raw)?;
    if value.get() < minimum {
        return Err(format!(
            "{key} must be at least {minimum} so configured health probes cannot exhaust it, got {}",
            value.get()
        ));
    }
    Ok(value)
}

fn parse_bounded_usize(
    key: &str,
    raw: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, String> {
    let value: usize = raw
        .trim()
        .parse()
        .map_err(|e| format!("{key} must be a whole number, got {raw:?}: {e}"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "{key} must be between {minimum} and {maximum}, got {raw:?}"
        ));
    }
    Ok(value)
}

fn parse_ip_networks(key: &str, raw: &str) -> Result<Vec<IpNet>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    raw.split(',')
        .map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return Err(format!("{key} contains an empty network in {raw:?}"));
            }
            part.parse::<IpNet>()
                .map_err(|e| format!("{key} contains an invalid IP network {part:?}: {e}"))
        })
        .collect()
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

fn parse_cutover_drain(key: &str, raw: &str) -> Result<bool, String> {
    match raw {
        "" => Ok(false),
        "drained" => Ok(true),
        _ => Err(format!(
            "{key} must be unset or exactly \"drained\" after every incompatible CMA process and its MongoDB and provider operations have stopped"
        )),
    }
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
            rate_limits: RateLimitConfig::sample(),
            trusted_proxies: TrustedProxyConfig::sample(),
            sandbox_base_url: "http://r".into(),
            sandbox_api_key: "k".into(),
            sandbox_image: "img".into(),
            sandbox_cpu: "1".into(),
            sandbox_memory: "1Gi".into(),
            default_agent: Default::default(),
            baseline_packages: vec![],
            baseline_skills: vec![],
            baseline_services: vec![],
            deployment_id: DEFAULT_DEPLOYMENT_ID.into(),
            adopts_untagged_runtimes: true,
            upstream_base_url: "http://u".into(),
            upstream_model: "m".into(),
            upstream_wire_api: "responses".into(),
            claude_upstream_base_url: "http://u".into(),
            claude_upstream_model: "m".into(),
            dsh_upstream_base_url: "http://u".into(),
            dsh_upstream_model: "m".into(),
            platform_base_url: "http://p".into(),
            mongodb_uri: "mongodb://h/db".into(),
            mongodb_database: "db".into(),
            mongodb_legacy_cutover: false,
            mongodb_coordination_cutover: false,
            auth_encryption_key: AuthEncryptionKey::from_base64(
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            )
            .expect("test key has 32 decoded bytes"),
            activity_retention: std::time::Duration::from_secs(90 * 86_400),
            context_bucket: "bkt".into(),
            trigger_workspace_bucket: "trigger-bkt".into(),
            trigger: TriggerRuntimeConfig::sample(),
            github_capability_signing: None,
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
    use std::collections::BTreeMap;

    use super::*;

    fn staged_config(values: &[(&str, &str)]) -> Result<Option<MongoCutoverStageConfig>, String> {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>();
        MongoCutoverStageConfig::from_lookup(|key| Ok(values.get(key).cloned()))
    }

    #[test]
    fn trigger_runtime_capabilities_accept_only_the_six_canonical_names() {
        let raw = "conversation,durable_conversation,workspace_checkpoint,restricted_service_gateway,credential_refresh,credential_revocation";
        assert_eq!(
            parse_trigger_runtime_capabilities("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", raw)
                .expect("the complete canonical inventory is usable"),
            all_trigger_runtime_capabilities()
        );
        assert_eq!(
            parse_trigger_runtime_capabilities("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", "")
                .expect("an empty declaration deliberately admits no Trigger capability"),
            Vec::<RequiredCapability>::new()
        );
    }

    #[test]
    fn trigger_runtime_capabilities_reject_duplicates_and_noncanonical_spelling() {
        for raw in [
            "conversation,conversation",
            " conversation",
            "conversation ",
            "conversation, durable_conversation",
            "Conversation",
            "unknown_capability",
        ] {
            let error =
                parse_trigger_runtime_capabilities("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", raw)
                    .expect_err("ambiguous or noncanonical capability input must fail startup");
            assert!(
                error.contains("CHRONO_TRIGGER_RUNTIME_CAPABILITIES"),
                "the startup error must name its deployment key: {error}"
            );
        }
    }

    #[test]
    fn a_blank_value_falls_back_to_the_default() {
        // An env var set to the empty string is a common deployment mistake and
        // must behave as absent rather than producing an empty bind address.
        assert_eq!(var_or("CHRONO_TEST_ABSENT_KEY", "fallback"), "fallback");
    }

    #[test]
    fn a_new_deployment_defaults_to_the_cma_identity() {
        assert_eq!(
            var_or("CHRONO_TEST_ABSENT_DEPLOYMENT_ID", DEFAULT_DEPLOYMENT_ID),
            "cma"
        );
    }

    #[test]
    fn a_new_deployment_defaults_to_the_cma_runtime_image() {
        assert_eq!(
            var_or("CHRONO_TEST_ABSENT_SANDBOX_IMAGE", DEFAULT_SANDBOX_IMAGE),
            "cma-runtime:dev"
        );
    }

    #[test]
    fn the_baseline_reaches_the_skill_runtime_and_not_only_its_registry() {
        // `ornn` and `ornn-api` are two services. The registry is what this
        // service searches to show a person what they may pick; the runtime is
        // what the agent calls once it has picked. A baseline naming only the
        // registry mints a credential the provider refuses on every call the
        // agent actually makes — the failure this pins.
        let baseline = list_or("CHRONO_TEST_ABSENT_SERVICES", BASELINE_SERVICES);
        assert!(
            baseline.iter().any(|s| s == "ornn"),
            "the skill runtime must be reachable, got {baseline:?}"
        );
        assert!(
            baseline.iter().any(|s| s == "ornn-api"),
            "the skill registry must stay reachable, got {baseline:?}"
        );
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
    fn request_rates_accept_only_positive_whole_numbers() {
        assert_eq!(
            parse_positive_u32("CHRONO_SERVER_RATE_CREATE_PER_MINUTE", " 17 ")
                .expect("a positive rate is usable")
                .get(),
            17
        );
        for raw in ["0", "-1", "1.5", "many", ""] {
            let error = parse_positive_u32("CHRONO_SERVER_RATE_CREATE_PER_MINUTE", raw)
                .expect_err("an unusable rate must fail startup");
            assert!(
                error.contains("CHRONO_SERVER_RATE_CREATE_PER_MINUTE"),
                "got {error}"
            );
        }
    }

    #[test]
    fn health_rate_cannot_starve_the_declared_probe_schedule() {
        assert_eq!(
            u32_at_least(
                "CHRONO_SERVER_RATE_HEALTH_PER_MINUTE",
                HEALTH_RATE_REQUESTS_PER_MINUTE,
                MINIMUM_HEALTH_RATE_REQUESTS_PER_MINUTE,
            )
            .unwrap()
            .get(),
            HEALTH_RATE_REQUESTS_PER_MINUTE
        );
        let error = parse_u32_at_least(
            "CHRONO_SERVER_RATE_HEALTH_PER_MINUTE",
            "29",
            MINIMUM_HEALTH_RATE_REQUESTS_PER_MINUTE,
        )
        .expect_err("a probe-starving budget must be refused");
        assert!(error.contains("at least 30"), "got {error}");
    }

    #[test]
    fn trusted_proxy_configuration_is_explicit_and_bounded() {
        let networks = parse_ip_networks(
            "CHRONO_SERVER_TRUSTED_PROXY_CIDRS",
            " 35.191.0.0/16, 2600:2d00:1:1::/64 ",
        )
        .expect("documented GFE networks parse");
        assert_eq!(networks.len(), 2);
        assert!(networks[0].contains(&"35.191.4.2".parse::<std::net::IpAddr>().unwrap()));
        assert!(networks[1].contains(&"2600:2d00:1:1::9".parse::<std::net::IpAddr>().unwrap()));
        assert!(parse_ip_networks("CIDRS", "").unwrap().is_empty());
        for raw in [",", "35.191.0.0/16,", "not-a-network"] {
            assert!(
                parse_ip_networks("CHRONO_SERVER_TRUSTED_PROXY_CIDRS", raw).is_err(),
                "accepted {raw:?}"
            );
        }

        assert_eq!(
            parse_bounded_usize("CHRONO_SERVER_TRUSTED_PROXY_HOPS", "1", 1, 16).unwrap(),
            1
        );
        for raw in ["0", "17", "many"] {
            assert!(
                parse_bounded_usize("CHRONO_SERVER_TRUSTED_PROXY_HOPS", raw, 1, 16).is_err(),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn cutover_gates_require_the_exact_post_drain_acknowledgement() {
        assert!(!parse_cutover_drain("CUTOVER", "").unwrap());
        assert!(parse_cutover_drain("CUTOVER", "drained").unwrap());
        for unsafe_value in ["true", "yes", "drain", "DRAINED", " drained", "drained "] {
            let error = parse_cutover_drain("CUTOVER", unsafe_value)
                .expect_err("an imprecise acknowledgement must stay closed");
            assert!(error.contains("exactly \"drained\""), "got {error}");
        }
    }

    #[test]
    fn trigger_rollout_values_fail_closed_instead_of_guessing() {
        assert!(strict_bool("TRIGGER_BOOL", "true").unwrap());
        assert!(!strict_bool("TRIGGER_BOOL", "false").unwrap());
        for raw in ["TRUE", "False", "1", "yes", " true", ""] {
            assert!(
                strict_bool("TRIGGER_BOOL", raw).is_err(),
                "accepted {raw:?}"
            );
        }

        assert_eq!(
            parse_u64_list("CHRONO_TRIGGER_REPOSITORY_IDS", " 17,42 ").unwrap(),
            vec![17, 42]
        );
        assert!(parse_u64_list("CHRONO_TRIGGER_REPOSITORY_IDS", "")
            .unwrap()
            .is_empty());
        for raw in ["0", "17,", "-1", "repo"] {
            assert!(
                parse_u64_list("CHRONO_TRIGGER_REPOSITORY_IDS", raw).is_err(),
                "accepted {raw:?}"
            );
        }

        assert_eq!(
            parse_trigger_cleanup_mode("CHRONO_TRIGGER_CLEANUP_MODE", "dry_run").unwrap(),
            TriggerCleanupMode::DryRun
        );
        assert_eq!(
            parse_trigger_cleanup_mode("CHRONO_TRIGGER_CLEANUP_MODE", "execute").unwrap(),
            TriggerCleanupMode::Execute
        );
        for raw in ["delete", "DRY_RUN", " dry_run", ""] {
            assert!(
                parse_trigger_cleanup_mode("CHRONO_TRIGGER_CLEANUP_MODE", raw).is_err(),
                "accepted {raw:?}"
            );
        }

        assert_eq!(
            parse_bounded_usize(
                "CHRONO_TRIGGER_CHANGED_FILE_LIMIT",
                "1000",
                1,
                layer2_core::MAX_DIFF_SUMMARY_FILES,
            )
            .unwrap(),
            1_000
        );
        for raw in ["0", "1001", "unbounded"] {
            assert!(
                parse_bounded_usize(
                    "CHRONO_TRIGGER_CHANGED_FILE_LIMIT",
                    raw,
                    1,
                    layer2_core::MAX_DIFF_SUMMARY_FILES,
                )
                .is_err(),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn the_legacy_cutover_stage_accepts_only_its_exact_flag_tuple() {
        let config = staged_config(&[
            (MONGODB_CUTOVER_STAGE, "legacy"),
            ("CHRONO_MONGODB_LEGACY_CUTOVER", "drained"),
            ("CHRONO_MONGODB_COORDINATION_CUTOVER", ""),
            ("CHRONO_MONGODB_URI", "mongodb://stage.invalid/cma"),
        ])
        .expect("the exact staged tuple is usable")
        .expect("the legacy stage is selected");
        assert_eq!(config.stage, MongoCutoverStage::Legacy);
        assert_eq!(config.bind, "0.0.0.0:8080");
        assert_eq!(config.mongodb_database, "chrono_bot");

        for stage in ["unknown", " Legacy", "legacy ", "LEGACY"] {
            let error = staged_config(&[(MONGODB_CUTOVER_STAGE, stage)])
                .expect_err("an unknown stage must fail before other configuration");
            assert!(error.contains(MONGODB_CUTOVER_STAGE), "got {error}");
        }
        let secret_stage = "mongodb://operator:do-not-log-this@invalid";
        let error = staged_config(&[(MONGODB_CUTOVER_STAGE, secret_stage)])
            .expect_err("an invalid stage is refused without echoing its value");
        assert!(!error.contains("do-not-log-this"), "got {error}");
        for legacy in ["", "drain", " drained", "drained ", "DRAINED"] {
            let error = staged_config(&[
                (MONGODB_CUTOVER_STAGE, "legacy"),
                ("CHRONO_MONGODB_LEGACY_CUTOVER", legacy),
            ])
            .expect_err("the stage requires an exact legacy drain acknowledgement");
            assert!(
                error.contains("CHRONO_MONGODB_LEGACY_CUTOVER=drained"),
                "got {error}"
            );
        }
        for coordination in ["drained", " ", "false"] {
            let error = staged_config(&[
                (MONGODB_CUTOVER_STAGE, "legacy"),
                ("CHRONO_MONGODB_LEGACY_CUTOVER", "drained"),
                ("CHRONO_MONGODB_COORDINATION_CUTOVER", coordination),
            ])
            .expect_err("coordination must remain structurally disabled in this stage");
            assert!(
                error.contains("CHRONO_MONGODB_COORDINATION_CUTOVER"),
                "got {error}"
            );
        }
    }

    #[test]
    fn every_trigger_runtime_key_is_pinned_in_the_fixed_configmap() {
        let manifest = include_str!("../../../../infra/api/configmap.yaml");
        for (key, value) in [
            ("CHRONO_TRIGGER_ENABLED", "false"),
            ("CHRONO_TRIGGER_ORCHESTRATION_PAUSED", "true"),
            ("CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH", "true"),
            ("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", ""),
            ("CHRONO_TRIGGER_PUBLISHER_SUBJECT_IDS", ""),
            ("CHRONO_TRIGGER_REPOSITORY_IDS", ""),
            ("CHRONO_TRIGGER_USER_COHORT_PERCENT", "0"),
            ("CHRONO_TRIGGER_CLEANUP_MODE", "dry_run"),
            ("CHRONO_TRIGGER_CHANGED_FILE_LIMIT", "1000"),
            ("CHRONO_TRIGGER_WORKER_INTERVAL_SECONDS", "5"),
            ("CHRONO_TRIGGER_WORKER_PAGE_SIZE", "50"),
            ("CHRONO_TRIGGER_WORKER_MAX_PAGES", "20"),
            ("CHRONO_TRIGGER_WORKER_CONCURRENCY", "4"),
        ] {
            let declaration = format!("  {key}: \"{value}\"");
            assert!(
                manifest.lines().any(|line| line == declaration),
                "fixed ConfigMap is missing {declaration}"
            );
        }
    }

    #[test]
    fn an_unselected_stage_observes_no_control_plane_configuration() {
        let mut accessed = Vec::new();
        let stage = MongoCutoverStageConfig::from_lookup(|key| {
            accessed.push(key.to_owned());
            Ok(None)
        })
        .expect("an absent stage is normal startup");
        assert!(stage.is_none());
        assert_eq!(accessed, [MONGODB_CUTOVER_STAGE]);
    }

    #[test]
    fn the_legacy_stage_reads_only_its_six_narrow_keys() {
        let values = BTreeMap::from([
            (MONGODB_CUTOVER_STAGE, "legacy"),
            ("CHRONO_MONGODB_LEGACY_CUTOVER", "drained"),
            ("CHRONO_MONGODB_COORDINATION_CUTOVER", ""),
            ("CHRONO_SERVER_BIND", "127.0.0.1:18080"),
            ("CHRONO_MONGODB_URI", "mongodb://stage.invalid/cma"),
            ("CHRONO_MONGODB_DATABASE", "stage_boundary"),
        ]);
        let mut accessed = Vec::new();
        let config = MongoCutoverStageConfig::from_lookup(|key| {
            accessed.push(key.to_owned());
            values
                .get(key)
                .map(|value| Some((*value).to_owned()))
                .ok_or_else(|| format!("stage read forbidden configuration key {key}"))
        })
        .expect("the narrow configuration is sufficient")
        .expect("stage is selected");
        assert_eq!(config.bind, "127.0.0.1:18080");
        assert_eq!(config.mongodb_database, "stage_boundary");
        assert_eq!(
            accessed,
            [
                MONGODB_CUTOVER_STAGE,
                "CHRONO_MONGODB_LEGACY_CUTOVER",
                "CHRONO_MONGODB_COORDINATION_CUTOVER",
                "CHRONO_SERVER_BIND",
                "CHRONO_MONGODB_URI",
                "CHRONO_MONGODB_DATABASE",
            ]
        );
    }

    #[test]
    fn the_stage_refuses_an_unreadable_environment_value() {
        let error = MongoCutoverStageConfig::from_lookup(|key| {
            Err(format!(
                "{key} is not valid UTF-8 and cannot be interpreted"
            ))
        })
        .expect_err("a non-text selector must not silently become normal startup");
        assert!(error.contains(MONGODB_CUTOVER_STAGE), "got {error}");
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
