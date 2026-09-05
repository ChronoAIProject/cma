use super::HostedAdapter;
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
        self.known_state(handle).await?;
        match self
            .exchange(handle, RequestFrame::Install(request.clone()))
            .await
        {
            Ok(ResponseFrame::Install(receipt)) => {
                validate_receipt(request, &receipt)?;
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
