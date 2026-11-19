use super::*;
use crate::hosted::{HostedAccessToolBinding, HostedProviderToolBinding};

#[derive(Clone)]
pub(in crate::hosted) enum ToolTransport {
    Access(HostedAccessToolBinding),
    Provider(Box<HostedProviderToolBinding>),
}

#[cfg(test)]
#[path = "ingress_tests.rs"]
mod ingress_tests;

impl ToolTransport {
    fn name(&self) -> &'static str {
        match self {
            Self::Access(_) => "cma_access",
            Self::Provider(_) => "cmaeg",
        }
    }

    fn server(&self) -> serde_json::Value {
        match self {
            Self::Access(binding) => binding.acp_server(),
            Self::Provider(binding) => binding.acp_server(),
        }
    }
}

impl HostedContextRefresh {
    pub(in crate::hosted) fn tools_frame(
        &self,
        policy: Option<&crate::ingress_budget::IngressOwnershipPolicy>,
    ) -> crate::Result<crate::hosted::retained_json::JsonFrame> {
        if policy.is_none() {
            return Ok(crate::hosted::retained_json::JsonFrame::legacy(
                serde_json::Value::Array(self.tools()),
            ));
        }
        self.tools_frame_for_transport(None, policy)
    }

    pub(in crate::hosted) fn tools_frame_for_transport(
        &self,
        transport: Option<&ToolTransport>,
        policy: Option<&crate::ingress_budget::IngressOwnershipPolicy>,
    ) -> crate::Result<crate::hosted::retained_json::JsonFrame> {
        use crate::hosted::{acp::ingress::typed::Shape, retained_json::JsonFrame};
        use crate::ingress_budget::{AllocationPlan, IngressClass};
        let Some(policy) = policy else {
            return Ok(JsonFrame::legacy(serde_json::Value::Array(
                self.tools_for_transport(transport)?,
            )));
        };
        // Keep one installed binding snapshot through sizing and construction.
        // Admission is nonwaiting and no async operation runs under this lock.
        let installed = self
            .0
            .installed_transport
            .read()
            .expect("transport state lock");
        let transport = transport.or(installed.as_ref());
        let replacement = if let Some(transport) = transport {
            let mut indices = self.0.tools.iter().enumerate().filter_map(|(index, tool)| {
                (tool.get("name").and_then(serde_json::Value::as_str) == Some(transport.name()))
                    .then_some(index)
            });
            let index = indices
                .next()
                .filter(|_| indices.next().is_none())
                .ok_or_else(|| {
                    AdapterError::Unsupported("managed tool connection is ambiguous".into())
                })?;
            Some((index, transport))
        } else {
            None
        };
        let descriptor = match transport {
            Some(ToolTransport::Access(binding)) => binding.acp_server_plan()?.requested_bytes(),
            Some(ToolTransport::Provider(binding)) => binding.acp_server_plan()?.requested_bytes(),
            None => 0,
        };
        let storage = policy.reserve(
            IngressClass::Data,
            AllocationPlan::new(
                "retained conversation tools",
                &[
                    Shape::serialized(&self.0.tools)?.retained::<serde_json::Value>()?,
                    descriptor,
                ],
            )?,
        )?;
        Ok(JsonFrame::admitted(storage.initialize(|| {
            let mut tools = self.0.tools.clone();
            if let Some((index, transport)) = replacement {
                tools[index] = transport.server();
            }
            serde_json::Value::Array(tools)
        })))
    }

    pub(in crate::hosted) async fn prepare_transport(
        &self,
        session: &str,
        sequence: u64,
        origin: Option<&str>,
    ) -> crate::Result<Option<ToolTransport>> {
        let Some(origin) = origin else {
            return Ok(None);
        };
        let installed = self
            .0
            .installed_transport
            .read()
            .expect("transport state lock")
            .clone();
        let result = match (&self.0.access, &self.0.provider, installed.as_ref()) {
            (Some((source, binding)), None, None) => source
                .prepare_transport(binding, origin, session, sequence)
                .await
                .map(ToolTransport::Access),
            (Some((source, _)), None, Some(ToolTransport::Access(binding))) => source
                .prepare_transport(binding, origin, session, sequence)
                .await
                .map(ToolTransport::Access),
            (None, Some((source, binding)), None) => source
                .prepare_transport(binding, origin, session, sequence)
                .await
                .map(|binding| ToolTransport::Provider(Box::new(binding))),
            (None, Some((source, _)), Some(ToolTransport::Provider(binding))) => source
                .prepare_transport(binding, origin, session, sequence)
                .await
                .map(|binding| ToolTransport::Provider(Box::new(binding))),
            _ => Err(AdapterError::Unsupported(
                "conversation has no unique tool transport authority".into(),
            )),
        };
        result.map(Some).map_err(|error| match error {
            AdapterError::Rejected(_) => AdapterError::Unsupported(
                "retained tool transport authority refused maintenance".into(),
            ),
            other => other,
        })
    }

    pub(in crate::hosted) fn tools_for_transport(
        &self,
        transport: Option<&ToolTransport>,
    ) -> crate::Result<Vec<serde_json::Value>> {
        let installed = self
            .0
            .installed_transport
            .read()
            .expect("transport state lock")
            .clone();
        let mut tools = self.0.tools.clone();
        if let Some(transport) = transport.or(installed.as_ref()) {
            let indices: Vec<_> = tools
                .iter()
                .enumerate()
                .filter_map(|(i, tool)| {
                    (tool.get("name").and_then(serde_json::Value::as_str) == Some(transport.name()))
                        .then_some(i)
                })
                .collect();
            if indices.len() != 1 {
                return Err(AdapterError::Unsupported(
                    "managed tool connection is ambiguous".into(),
                ));
            }
            tools[indices[0]] = transport.server();
        }
        Ok(tools)
    }

    pub(in crate::hosted) async fn confirm_transport(
        &self,
        transport: &ToolTransport,
        session: &str,
        sequence: u64,
    ) -> crate::Result<()> {
        match (transport, &self.0.access, &self.0.provider) {
            (ToolTransport::Access(binding), Some((source, _)), None) => {
                source.confirm_transport(binding, session, sequence).await?;
            }
            (ToolTransport::Provider(binding), None, Some((source, _))) => {
                source.confirm_transport(binding, session, sequence).await?;
            }
            _ => {
                return Err(AdapterError::Unavailable(
                    "tool confirmation source is unavailable".into(),
                ))
            }
        }
        *self
            .0
            .installed_transport
            .write()
            .expect("transport state lock") = Some(transport.clone());
        Ok(())
    }
}
