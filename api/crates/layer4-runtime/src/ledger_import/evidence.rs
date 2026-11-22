//! Read-only V1 mutation facts. Native command correlation remains unknown.
mod output;
mod records;
mod source;

use std::fs::File;
use std::io::Write;

use super::{changed, inventory, validation, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

const OUTPUT_BYTES: usize = 16 * 1024 * 1024;

pub(crate) async fn command() -> std::result::Result<(), String> {
    let config = RuntimeConfig::from_environment()
        .map_err(|_| "ledger_evidence_configuration_invalid".to_owned())?;
    let bytes = tokio::task::spawn_blocking(move || {
        collect(
            &config,
            &mut Budget::new(Limits::default()),
            OUTPUT_BYTES,
            source::readonly,
            |_| Ok(()),
        )
    })
    .await
    .map_err(|_| "ledger_evidence_worker_failed".to_owned())?
    .map_err(|failure| failure.code)?;
    publish(&bytes, &mut std::io::stdout().lock()).map_err(|failure| failure.code)
}

fn publish(bytes: &[u8], sink: &mut impl Write) -> Result<()> {
    sink.write_all(bytes)
        .and_then(|_| sink.flush())
        .map_err(|_| error("ledger_evidence_output_io"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Boundary {
    Validated,
    Projected,
}

pub(super) fn collect(
    config: &RuntimeConfig,
    budget: &mut Budget,
    output_limit: usize,
    mount_check: fn(&File) -> Result<()>,
    mut boundary: impl FnMut(Boundary) -> Result<()>,
) -> Result<Vec<u8>> {
    source::preflight(config, budget, mount_check)?;
    let inventory = inventory::scan_budget(config, budget)?;
    source::verify(&inventory, budget, mount_check)?;
    // The existing validator checks complete histories, including replacements.
    // Export every physical record below, not only its folded survivors.
    let analysis = validation::analyze_shared(config, &inventory, budget)?;
    drop(analysis);
    boundary(Boundary::Validated)?;
    let bytes = output::encode(config, &inventory, budget, output_limit)?;
    boundary(Boundary::Projected)?;
    source::preflight(config, budget, mount_check)?;
    if inventory::scan_budget(config, budget)?.digest != inventory.digest {
        return Err(changed());
    }
    source::verify(&inventory, budget, mount_check)?;
    budget.check()?;
    Ok(bytes)
}

fn error(code: &'static str) -> RuntimeError {
    RuntimeError::rejected(code, "V1 ledger evidence collection did not complete")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_failure_is_not_success() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            publish(b"{}\n", &mut Broken).unwrap_err().code,
            "ledger_evidence_output_io"
        );
    }
}
