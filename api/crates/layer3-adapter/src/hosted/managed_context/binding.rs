use super::{Binding, HostedContextRefresh, Shared};
use crate::hosted::{
    acp::ingress::typed::Shape, HostedAccessToolBinding, HostedAccessToolSource, HostedBackend,
    HostedProviderToolBinding, HostedProviderToolSource,
};
use crate::ingress_budget::{
    sizing, AdmissionRefusal, AdmittedError, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    SharedOwned,
};
use crate::{AdapterError, RuntimeHandle, RuntimeStatusRequest};

pub(super) enum Initial<T> {
    Legacy(T),
    Admitted(SharedOwned<T>),
}

impl<T> std::ops::Deref for Initial<T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Legacy(value) => value,
            Self::Admitted(value) => value,
        }
    }
}

impl<T> Initial<T> {
    fn belongs_to(&self, policy: &IngressOwnershipPolicy) -> bool {
        matches!(self, Self::Admitted(value) if value.belongs_to(policy))
    }
}

type Access = (
    SharedOwned<dyn HostedAccessToolSource>,
    SharedOwned<HostedAccessToolBinding>,
);
type Provider = (
    SharedOwned<dyn HostedProviderToolSource>,
    SharedOwned<HostedProviderToolBinding>,
);

pub(in crate::hosted) struct ContextSources {
    pub(in crate::hosted) backend: SharedOwned<dyn HostedBackend>,
    pub(in crate::hosted) access: Option<Access>,
    pub(in crate::hosted) provider: Option<Provider>,
}

pub(in crate::hosted) struct ContextInput<'a> {
    pub(in crate::hosted) handle: &'a RuntimeHandle,
    pub(in crate::hosted) identity: &'a RuntimeStatusRequest,
    pub(in crate::hosted) home: &'a str,
    pub(in crate::hosted) cwd: &'a str,
    pub(in crate::hosted) tools: &'a [serde_json::Value],
}

fn invalid() -> AdmissionRefusal {
    AdmissionRefusal::InvalidPlan {
        phase: "context binding ownership",
    }
}

impl HostedContextRefresh {
    /// Borrowed originals keep their upstream owner; sources and optional
    /// bindings must already own all their storage under this attachment.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "admitted construction is supplied by the transport package"
        )
    )]
    pub(in crate::hosted) fn new_admitted(
        policy: &IngressOwnershipPolicy,
        sources: ContextSources,
        input: ContextInput<'_>,
    ) -> Result<Self, AdmittedError> {
        if !sources.backend.belongs_to(policy)
            || sources.access.as_ref().is_some_and(|(source, binding)| {
                !source.belongs_to(policy) || !binding.belongs_to(policy)
            })
            || sources.provider.as_ref().is_some_and(|(source, binding)| {
                !source.belongs_to(policy) || !binding.belongs_to(policy)
            })
        {
            return Err(invalid().into());
        }
        // Existing domain validation allocates fixed diagnostics on failure.
        // Retain that branch before validation; successful sizing allocates none.
        let validation = policy.admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "context binding validation",
                &[
                    sizing::shared_owner_bytes::<AdapterError>(),
                    "Agent instructions require a retained complete home"
                        .len()
                        .max("runtime status identity is invalid".len())
                        .max("typed ingress layout exceeds supported bounds".len())
                        .max("ingress allocation plan exceeds its supported bounds".len()),
                ],
            )?,
        )?;
        let validated = (|| {
            let path = input
                .home
                .strip_prefix("/home/")
                .filter(|path| crate::valid_agent_file_path(path))
                .ok_or_else(|| {
                    AdapterError::Unsupported(
                        "Agent instructions require a retained complete home".into(),
                    )
                })?;
            input.identity.validate()?;
            let tools = Shape::serialized(&input.tools)?.retained::<Vec<serde_json::Value>>()?;
            Ok::<_, AdapterError>((path, tools))
        })();
        let (path, tools) = match validated {
            Ok(value) => value,
            Err(error) => return Err(validation.initialize(|| error).into()),
        };
        let path_bytes = path
            .len()
            .checked_add("/AGENTS.md".len())
            .ok_or_else(invalid)?;
        let storage = policy.admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "initial context binding",
                &[
                    sizing::shared_owner_bytes::<Binding>(),
                    input.handle.as_str().len(),
                    input.identity.sandbox_id.len(),
                    input.identity.home_identity.len(),
                    path_bytes,
                    input.cwd.len(),
                    tools,
                ],
            )?,
        )?;
        let binding = storage.initialize(|| {
            let mut owned_path = String::with_capacity(path_bytes);
            owned_path.push_str(path);
            owned_path.push_str("/AGENTS.md");
            Binding {
                backend: Shared::Admitted(sources.backend),
                handle: input.handle.clone(),
                identity: input.identity.clone(),
                path: owned_path,
                cwd: input.cwd.to_owned(),
                tools: input.tools.to_vec(),
                installed_transport: std::sync::RwLock::new(None),
                access: sources.access.map(|(source, binding)| {
                    (Shared::Admitted(source), Initial::Admitted(binding))
                }),
                provider: sources.provider.map(|(source, binding)| {
                    (Shared::Admitted(source), Initial::Admitted(binding))
                }),
            }
        });
        drop(validation);
        Ok(Self(Shared::Admitted(binding)))
    }

    pub(in crate::hosted) fn require_owner(
        &self,
        policy: &IngressOwnershipPolicy,
    ) -> Result<(), AdmissionRefusal> {
        if !self.0.belongs_to(policy)
            || !self.0.backend.belongs_to(policy)
            || self.0.access.as_ref().is_some_and(|(source, binding)| {
                !source.belongs_to(policy) || !binding.belongs_to(policy)
            })
            || self.0.provider.as_ref().is_some_and(|(source, binding)| {
                !source.belongs_to(policy) || !binding.belongs_to(policy)
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}
