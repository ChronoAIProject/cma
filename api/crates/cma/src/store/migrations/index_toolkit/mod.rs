//! Explicit production inspection and online Activity index-build commands.
//!
//! These commands are separate from startup and HTTP reads. Sizing is read-only;
//! the build path uses the durable migration lease and manifest with a bounded DDL.

use std::time::Duration;

use mongodb::{options::ClientOptions, Client, Database};
use serde_json::Value;

use super::{Result, IO};

mod build;
mod inspection;
use build::build;
use inspection::size;

const COLLECTION: &str = "activity";
const SIZE_DEADLINE: Duration = Duration::from_secs(30);
const DEFAULT_BUILD_TIMEOUT_SECONDS: u64 = 2_100;
const MIN_BUILD_TIMEOUT_SECONDS: u64 = 60;
const MAX_BUILD_TIMEOUT_SECONDS: u64 = 3_600;
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
const LEASE_RENEWAL_INTERVAL: Duration = Duration::from_secs(5);

enum Operation {
    Size,
    Build {
        checksum: String,
        image: String,
        timeout_seconds: u64,
    },
}

pub(super) fn is_command(arguments: &[String]) -> bool {
    matches!(
        arguments.first().map(String::as_str),
        Some("--activity-index-size") | Some("--activity-index-build")
    )
}

pub(super) async fn command(arguments: &[String]) -> Result<Value> {
    let operation = parse(arguments)?;
    match operation {
        Operation::Size => tokio::time::timeout(SIZE_DEADLINE, size())
            .await
            .map_err(|_| "activity sizing deadline exceeded; no write was attempted")?,
        Operation::Build {
            checksum,
            image,
            timeout_seconds,
        } => tokio::time::timeout(
            Duration::from_secs(timeout_seconds.saturating_add(60)),
            build(&checksum, &image, timeout_seconds),
        )
        .await
        .map_err(|_| "activity index deadline exceeded; inspect index and migration status")?,
    }
}

fn parse(arguments: &[String]) -> Result<Operation> {
    match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--activity-index-size"] => Ok(Operation::Size),
        ["--activity-index-build", checksum, image] => Ok(Operation::Build {
            checksum: (*checksum).to_owned(),
            image: (*image).to_owned(),
            timeout_seconds: DEFAULT_BUILD_TIMEOUT_SECONDS,
        }),
        ["--activity-index-build", checksum, image, timeout] => Ok(Operation::Build {
            checksum: (*checksum).to_owned(),
            image: (*image).to_owned(),
            timeout_seconds: parse_timeout(timeout)?,
        }),
        _ => Err("unsupported Activity index command or arguments"),
    }
}

fn parse_timeout(value: &str) -> Result<u64> {
    let seconds = value
        .parse::<u64>()
        .map_err(|_| "Activity index timeout must be an integer number of seconds")?;
    if !(MIN_BUILD_TIMEOUT_SECONDS..=MAX_BUILD_TIMEOUT_SECONDS).contains(&seconds) {
        return Err("Activity index timeout must be between 60 and 3600 seconds");
    }
    Ok(seconds)
}

async fn open_database() -> Result<(
    Client,
    Database,
    cma_service_auth::mongodb_authority::MongoAuthorityMode,
)> {
    let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database is required")?;
    validate_database(&name)?;
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| "database URI is required")?;
    let mode = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
        "CHRONO_MONGODB_AUTHORITY_MODE",
    )?;
    let mut options = ClientOptions::parse(uri)
        .await
        .map_err(|_| "database configuration")?;
    options.server_selection_timeout = Some(IO);
    options.connect_timeout = Some(IO);
    let client = Client::with_options(options).map_err(|_| "database configuration")?;
    Ok((client.clone(), client.database(&name), mode))
}

fn validate_database(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 63
        || name
            .chars()
            .any(|c| c.is_whitespace() || "/\\.\"$\0".contains(c))
    {
        return Err("invalid database name");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_keeps_sizing_read_only_and_bounds_build_timeout() {
        assert!(matches!(
            parse(&["--activity-index-size".into()]),
            Ok(Operation::Size)
        ));
        assert!(matches!(
            parse(&[
                "--activity-index-build".into(),
                "a".repeat(64),
                "ghcr.io/chronoaiproject/cma-api@sha256:".to_owned() + &"b".repeat(64),
                "60".into(),
            ]),
            Ok(Operation::Build {
                timeout_seconds: 60,
                ..
            })
        ));
        assert!(parse_timeout("59").is_err());
        assert!(parse_timeout("3601").is_err());
        assert_eq!(parse_timeout("2100").unwrap(), 2100);
    }
}
