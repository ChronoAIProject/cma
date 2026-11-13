// Frozen reader from 9827b2bdb; only module references are redirected for this compatibility test.
use layer3_adapter::{InstallReceipt, InstallRequest, RestoreBootstrapReceipt};

use super::v1_reader::{validate_request, InstallRecord, RECORD_VERSION};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

pub(crate) fn validate_record(config: &RuntimeConfig, record: &InstallRecord) -> Result<()> {
    let (version, sandbox_id) = record.identity();
    if version != RECORD_VERSION || sandbox_id != config.sandbox_id {
        return Err(invalid(
            "installation state version or sandbox identity does not match this runtime",
        ));
    }
    match record {
        InstallRecord::Started { request, .. } | InstallRecord::Indeterminate { request, .. } => {
            validate_request(request).map_err(|_| invalid("stored install request is invalid"))
        }
        InstallRecord::Completed {
            request, receipt, ..
        } if validate_request(request).is_ok() && valid_receipt(request, receipt) => Ok(()),
        InstallRecord::Completed { .. } => Err(invalid(
            "completed installation state contains an invalid receipt",
        )),
        InstallRecord::Failed {
            request,
            code,
            detail,
            retryable,
            ..
        } if validate_request(request).is_ok() && valid_failure(code, detail, *retryable) => Ok(()),
        InstallRecord::Failed { .. } => Err(invalid(
            "installation state contains an invalid terminal failure",
        )),
        InstallRecord::Inherited { receipt, .. } if valid_bootstrap_receipt(receipt) => Ok(()),
        InstallRecord::Inherited { .. } => Err(invalid(
            "inherited installation state contains an invalid bootstrap receipt",
        )),
    }
}

fn valid_bootstrap_receipt(receipt: &RestoreBootstrapReceipt) -> bool {
    !receipt.operation_id.trim().is_empty()
        && receipt.operation_id.len() <= 256
        && !receipt.operation_id.chars().any(char::is_control)
        && lower_sha256(&receipt.request_sha256)
        && lower_sha256(&receipt.restore_request_sha256)
        && receipt.installation_inherited
}

fn valid_receipt(request: &InstallRequest, receipt: &InstallReceipt) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_sha256 == request.request_sha256
        && receipt.commands.len() == request.commands.len()
        && receipt.commands.iter().enumerate().all(|(index, receipt)| {
            receipt.index == u32::try_from(index).unwrap_or(u32::MAX)
                && receipt.exit_code == 0
                && lower_sha256(&receipt.stdout_sha256)
                && lower_sha256(&receipt.stderr_sha256)
        })
}

fn valid_failure(code: &str, detail: &str, retryable: bool) -> bool {
    !retryable
        && !code.is_empty()
        && code.len() <= 128
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && !detail.is_empty()
        && detail.chars().count() <= 1_024
        && detail
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
}

fn lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(detail: impl Into<String>) -> RuntimeError {
    RuntimeError::rejected("install_state_invalid", detail)
}
