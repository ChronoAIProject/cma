use super::{HostedAdapter, Phase};
use crate::hosted::reply::validate_command_receipts;
use crate::wire::{RequestFrame, ResponseFrame};
use crate::{AdapterError, InstallReceipt, InstallRequest, RuntimeHandle};

impl HostedAdapter {
    pub(super) async fn run_install_request(
        &self,
        handle: &RuntimeHandle,
        request: &InstallRequest,
    ) -> crate::Result<InstallReceipt> {
        validate_request(request)?;
        {
            let mut states = self.states.lock().await;
            let state = states.get_mut(handle).ok_or_else(|| {
                AdapterError::Unavailable(format!("unknown runtime {}", handle.0))
            })?;
            let identity = (&request.operation_id, &request.request_sha256);
            if state
                .install
                .as_ref()
                .is_some_and(|started| (&started.0, &started.1) != identity)
            {
                return Err(AdapterError::Rejected(
                    "install replay differs from the first request".into(),
                ));
            }
            if state.install.is_none() && state.phase != Phase::Provisioned {
                return Err(AdapterError::Rejected(
                    "install cannot begin after credential delivery".into(),
                ));
            }
            if state.phase == Phase::Provisioned {
                state.phase = Phase::Installing;
                state.install =
                    Some((request.operation_id.clone(), request.request_sha256.clone()));
            }
        }

        let response = self
            .exchange(handle, RequestFrame::Install(request.clone()))
            .await;
        let mut states = self.states.lock().await;
        let state = states.get_mut(handle).ok_or_else(|| {
            AdapterError::Unavailable(format!("runtime {} disappeared during install", handle.0))
        })?;
        match response {
            Ok(ResponseFrame::Install(receipt)) => {
                validate_receipt(request, &receipt)?;
                if state.phase == Phase::Installing {
                    state.phase = Phase::Installed;
                }
                Ok(receipt)
            }
            Ok(_) => unreachable!("exchange verifies response operation"),
            Err(error) => Err(error),
        }
    }
}

fn validate_request(request: &InstallRequest) -> crate::Result<()> {
    if request.operation_id.trim().is_empty()
        || request.operation_id.len() > 256
        || request.operation_id.chars().any(char::is_control)
        || !request.has_valid_sha256()
        || request.timeout_seconds == 0
        || request.output_limit_bytes == 0
    {
        return Err(AdapterError::Rejected(
            "install operation, digest, or bounds are invalid".into(),
        ));
    }
    Ok(())
}

fn validate_receipt(request: &InstallRequest, receipt: &InstallReceipt) -> crate::Result<()> {
    if receipt.operation_id != request.operation_id
        || receipt.request_sha256 != request.request_sha256
    {
        return Err(AdapterError::Rejected(
            "runtime returned a receipt for another install operation".into(),
        ));
    }
    validate_command_receipts(&receipt.commands, request.commands.len())
}
