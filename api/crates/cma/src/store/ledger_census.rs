//! Explicit read-only cohort census; no service startup, writes or effect authority.
pub(in crate::store) mod files;
mod observation;
mod report;
#[cfg(test)]
mod tests;

use std::{collections::BTreeSet, path::Path, time::Duration};

use layer2_core::sandbox::ledger_census::LedgerCensusAssessment;
use mongodb::{options::ClientOptions, Client};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, &'static str>;
const INVALID: &str = "ledger census input or authoritative snapshot is unavailable";
const OUTPUT: &str = "ledger census requires private create-only artifact paths";
const MAX_AGENTS: usize = 256;
const MAX_REQUEST: u64 = 256 * 1024;
const MAX_SOURCE: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub format: String,
    pub agents: Vec<Member>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Member {
    pub agent_id: String,
    pub sandbox_id: String,
}

impl Request {
    fn validate(&self) -> Result<()> {
        let valid = |id: &str| {
            !id.is_empty()
                && id.len() <= 256
                && id.trim() == id
                && !id.chars().any(char::is_control)
        };
        let mut seen = BTreeSet::new();
        if self.format != "runtime-ledger-census-request-v1"
            || self.agents.is_empty()
            || self.agents.len() > MAX_AGENTS
            || self
                .agents
                .iter()
                .any(|m| !valid(&m.agent_id) || !valid(&m.sandbox_id) || !seen.insert(&m.agent_id))
        {
            return Err(INVALID);
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub(super) struct Row {
    pub member: Member,
    pub outcome: &'static str,
    pub assessment: Option<LedgerCensusAssessment>,
    pub source_sha256: String,
}

#[derive(Debug, Serialize)]
pub(super) struct Census {
    pub format: &'static str,
    pub observed_at: String,
    pub request_sha256: String,
    pub source_sha256: String,
    pub rows: Vec<Row>,
    pub writes: bool,
    pub recovery_authorized: bool,
}

pub(crate) async fn command(request: &Path, output: &Path) -> Result<()> {
    let input = files::read(request)?;
    let request: Request = serde_json::from_slice(&input).map_err(|_| INVALID)?;
    request.validate()?;
    files::require_parent(output)?;
    let census = tokio::time::timeout(Duration::from_secs(60), async {
        let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| INVALID)?;
        let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| INVALID)?;
        let mode = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
            "CHRONO_MONGODB_AUTHORITY_MODE",
        )
        .map_err(|_| INVALID)?;
        let mut options = ClientOptions::parse(uri).await.map_err(|_| INVALID)?;
        super::transaction_barrier::bind(&mut options, &name);
        options.server_selection_timeout = Some(Duration::from_secs(5));
        options.connect_timeout = Some(Duration::from_secs(5));
        let client = Client::with_options(options).map_err(|_| INVALID)?;
        let database = client.database(&name);
        super::isolation::verify_with_authority(&database, mode)
            .await
            .map_err(|_| INVALID)?;
        observe(&client, &database, &request, digest(&input)).await
    })
    .await
    .map_err(|_| INVALID)??;
    let bytes = serde_json::to_vec(&census).map_err(|_| OUTPUT)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(OUTPUT);
    }
    files::publish(output, &bytes)?;
    println!("{}", report::summary(&census, &bytes));
    Ok(())
}

pub(super) use observation::observe;

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
