use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

macro_rules! labels {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
        pub enum $name { $(#[serde(rename = $label)] $variant),+ }

        impl $name {
            pub const fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),+ }
            }
        }
    };
}

labels!(Check {
    Web => "web_assets", Discovery => "oidc_discovery", Browser => "browser_session",
    Agent => "agent_read", AguiEvents => "agui_events", GatewayEvents => "gateway_events",
    Agui => "agui", Responses => "responses_v2", Gateway => "cmaeg",
});
labels!(Mode { Read => "read", Deterministic => "deterministic", Funded => "funded" });
labels!(Reason {
    Passed => "passed", Transport => "transport", Deadline => "timeout", Http => "http_status",
    CredentialMissing => "credential_missing", CredentialExpired => "credential_expired",
    CredentialDenied => "credential_denied", Contract => "contract", UnknownSend => "unknown_submission",
    ReceiptMissing => "receipt_missing", TerminalMissing => "terminal_missing",
    TerminalFailed => "terminal_failed", DeliveryLost => "delivery_lost", Missed => "missed",
    Budget => "budget_exhausted", Busy => "target_unsettled", State => "state_unavailable",
});
labels!(Phase { First => "first", Verify => "verification", Reconcile => "reconciliation", Provisional => "provisional" });

impl Check {
    pub const fn consumer(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Discovery => "nyxid",
            Self::Browser | Self::Agent => "cma_agent_owner",
            Self::Agui | Self::AguiEvents => "agui",
            Self::Responses => "responses_v2",
            Self::Gateway | Self::GatewayEvents => "cmaeg",
        }
    }

    pub const fn transport(self) -> &'static str {
        match self {
            Self::Web => "public_web",
            Self::Discovery => "oidc",
            Self::Browser => "browser_cookie",
            Self::Gateway => "signed_ingress",
            _ => "owner_bearer",
        }
    }
}

impl Mode {
    pub const fn interval(self) -> i64 {
        match self {
            Self::Read => 30,
            Self::Deterministic => 300,
            Self::Funded => 900,
        }
    }

    pub const fn deadline(self) -> i64 {
        match self {
            Self::Read => 10,
            Self::Deterministic => 30,
            Self::Funded => 120,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Job {
    pub check: Check,
    pub mode: Mode,
    pub offset: i64,
}

impl Job {
    pub fn key(self) -> String {
        format!("{}.{}", self.check.label(), self.mode.label())
    }

    pub fn slot_at(self, clock: i64) -> i64 {
        clock - (clock - self.offset).rem_euclid(self.mode.interval())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Intent {
    pub job: Job,
    pub slot: i64,
    pub key: String,
    pub target: String,
    pub body: String,
    pub marker: Option<String>,
    #[serde(flatten)]
    pub provenance: Provenance,
    pub request_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Provenance {
    pub configuration: String,
    pub candidate: String,
    pub runner_sha: String,
}

impl Intent {
    pub fn key(domain: &str, job: Job, slot: i64) -> String {
        let digest = Sha256::digest(format!("cma-journey/v1\n{domain}\n{}\n{slot}", job.key()));
        format!("journey-{digest:x}")
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Evidence {
    pub command: Option<String>,
    pub response: Option<String>,
    pub conversation: Option<String>,
    pub receipt_at: Option<i64>,
    pub terminal_at: Option<i64>,
    pub delivery_at: Option<i64>,
    pub result_digest: Option<String>,
    pub http_status: Option<u16>,
    pub delivery_elapsed_ms: Option<u64>,
    pub verification_elapsed_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Observation {
    pub reason: Reason,
    pub phase: Phase,
    pub completed_at: i64,
    pub elapsed_ms: u64,
    pub settled: bool,
    pub evidence: Evidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation_runner_sha: Option<String>,
}

pub fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
