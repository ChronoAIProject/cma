use super::RefreshRequest;
use crate::hosted::managed_context::{transport::ToolTransport, HostedContextRefresh};
use crate::ingress_budget::{diagnostic::OperationError, AdmittedError, IngressOwnershipPolicy};
use crate::AdapterError;
use std::future::Future;

pub(super) trait Mode: Sync {
    type Error: From<AdapterError> + Send;

    fn prepare(
        &self,
        binding: &HostedContextRefresh,
        request: &RefreshRequest<'_>,
    ) -> impl Future<Output = Result<Option<ToolTransport>, Self::Error>> + Send;
    fn goal_absent(
        &self,
        binding: &HostedContextRefresh,
        session: &str,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn append(
        &self,
        binding: &HostedContextRefresh,
        request: &RefreshRequest<'_>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn confirm(
        &self,
        binding: &HostedContextRefresh,
        transport: &ToolTransport,
        request: &RefreshRequest<'_>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub(super) struct Legacy;
impl Mode for Legacy {
    type Error = AdapterError;
    async fn prepare(
        &self,
        binding: &HostedContextRefresh,
        request: &RefreshRequest<'_>,
    ) -> crate::Result<Option<ToolTransport>> {
        binding
            .prepare_transport(
                request.session,
                request.operation_sequence,
                request.service_origin,
            )
            .await
    }
    async fn goal_absent(
        &self,
        binding: &HostedContextRefresh,
        session: &str,
    ) -> crate::Result<()> {
        binding.transport_goal_absent(session).await
    }
    async fn append(
        &self,
        binding: &HostedContextRefresh,
        request: &RefreshRequest<'_>,
    ) -> crate::Result<()> {
        binding
            .append_guidance(
                request.session,
                request.operation_sequence,
                request.readiness_receipt,
                request.guidance,
                request.previous_guidance,
            )
            .await
    }
    async fn confirm(
        &self,
        binding: &HostedContextRefresh,
        transport: &ToolTransport,
        request: &RefreshRequest<'_>,
    ) -> crate::Result<()> {
        binding
            .confirm_transport(transport, request.session, request.operation_sequence)
            .await
    }
}

pub(super) struct Admitted<'a>(pub(super) &'a IngressOwnershipPolicy);
impl Mode for Admitted<'_> {
    type Error = OperationError;
    async fn prepare(
        &self,
        binding: &HostedContextRefresh,
        request: &RefreshRequest<'_>,
    ) -> Result<Option<ToolTransport>, OperationError> {
        Ok(binding
            .prepare_transport_admitted(
                request.session,
                request.operation_sequence,
                request.service_origin,
                self.0,
            )
            .await?)
    }
    async fn goal_absent(
        &self,
        binding: &HostedContextRefresh,
        session: &str,
    ) -> Result<(), OperationError> {
        let absent = binding
            .transport_goal_absent_strict_admitted(session, self.0)?
            .await?;
        if !*absent {
            return Err(AdmittedError::Unsupported(
                "retained native goal state does not permit transport refresh",
            )
            .into());
        }
        Ok(())
    }
    async fn append(
        &self,
        binding: &HostedContextRefresh,
        request: &RefreshRequest<'_>,
    ) -> Result<(), OperationError> {
        binding
            .append_guidance_admitted(
                request.session,
                request.operation_sequence,
                request.readiness_receipt,
                request.guidance,
                request.previous_guidance,
                self.0,
            )?
            .await?;
        Ok(())
    }
    async fn confirm(
        &self,
        binding: &HostedContextRefresh,
        transport: &ToolTransport,
        request: &RefreshRequest<'_>,
    ) -> Result<(), OperationError> {
        Ok(binding
            .confirm_transport_admitted(
                transport,
                request.session,
                request.operation_sequence,
                self.0,
            )
            .await?)
    }
}
