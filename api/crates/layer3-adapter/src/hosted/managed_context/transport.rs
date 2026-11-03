use super::*;
use crate::hosted::HostedAccessToolBinding;

impl HostedContextRefresh {
    pub(in crate::hosted) async fn prepare_transport(
        &self,
        session: &str,
        sequence: u64,
        origin: Option<&str>,
    ) -> crate::Result<Option<HostedAccessToolBinding>> {
        let Some(origin) = origin else {
            return Ok(None);
        };
        let (source, binding) = self.0.access.as_ref().ok_or_else(|| {
            AdapterError::Unsupported("conversation has no managed access tool connection".into())
        })?;
        let installed = self
            .0
            .installed_transport
            .read()
            .expect("transport state lock")
            .clone();
        source
            .prepare_transport(
                installed.as_ref().unwrap_or(binding),
                origin,
                session,
                sequence,
            )
            .await
            .map(Some)
            .map_err(|error| match error {
                AdapterError::Rejected(_) => AdapterError::Unsupported(
                    "retained tool transport authority refused maintenance".into(),
                ),
                other => other,
            })
    }

    pub(in crate::hosted) fn tools_for_transport(
        &self,
        transport: Option<&HostedAccessToolBinding>,
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
                    (tool.get("name").and_then(serde_json::Value::as_str) == Some("cma_access"))
                        .then_some(i)
                })
                .collect();
            if indices.len() != 1 {
                return Err(AdapterError::Unsupported(
                    "managed tool connection is ambiguous".into(),
                ));
            }
            tools[indices[0]] = transport.acp_server();
        }
        Ok(tools)
    }

    pub(in crate::hosted) async fn confirm_transport(
        &self,
        transport: &HostedAccessToolBinding,
        session: &str,
        sequence: u64,
    ) -> crate::Result<()> {
        let (source, _) = self.0.access.as_ref().ok_or_else(|| {
            AdapterError::Unavailable(
                "managed access tool confirmation source is unavailable".into(),
            )
        })?;
        source
            .confirm_transport(transport, session, sequence)
            .await?;
        *self
            .0
            .installed_transport
            .write()
            .expect("transport state lock") = Some(transport.clone());
        Ok(())
    }
}
