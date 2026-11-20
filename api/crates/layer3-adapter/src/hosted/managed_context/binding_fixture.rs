use super::binding::{ContextInput, ContextSources};
use super::*;
use crate::ingress_budget::{
    sizing, AllocationPlan, IngressClass, IngressOwnershipPolicy, SharedOwned,
};

pub(in crate::hosted) fn source<T>(
    policy: &IngressOwnershipPolicy,
    heap_bytes: usize,
    initialize: impl FnOnce() -> T,
) -> SharedOwned<T> {
    policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "synthetic context source storage",
                &[sizing::shared_owner_bytes::<T>(), heap_bytes],
            )
            .unwrap(),
        )
        .unwrap()
        .initialize(initialize)
}

pub(in crate::hosted) fn context(
    policy: &IngressOwnershipPolicy,
    backend: SharedOwned<dyn HostedBackend>,
) -> HostedContextRefresh {
    HostedContextRefresh::new_admitted(
        policy,
        ContextSources {
            backend,
            access: None,
            provider: None,
        },
        ContextInput {
            handle: &RuntimeHandle("synthetic-runtime".into()),
            identity: &RuntimeStatusRequest {
                sandbox_id: "synthetic-sandbox".into(),
                generation: 3,
                home_identity: "h".repeat(43),
            },
            home: "/home/cma/.cma/agents/synthetic/home",
            cwd: "/home/cma/work",
            tools: &[],
        },
    )
    .unwrap()
}

pub(in crate::hosted) fn sources(context: &HostedContextRefresh) -> ContextSources {
    let Shared::Admitted(backend) = &context.0.backend else {
        panic!("owned fixture backend")
    };
    let access = context.0.access.as_ref().map(|(source, value)| {
        let (Shared::Admitted(source), Initial::Admitted(value)) = (source, value) else {
            panic!("owned access fixture")
        };
        (source.clone(), value.clone())
    });
    let provider = context.0.provider.as_ref().map(|(source, value)| {
        let (Shared::Admitted(source), Initial::Admitted(value)) = (source, value) else {
            panic!("owned provider fixture")
        };
        (source.clone(), value.clone())
    });
    ContextSources {
        backend: backend.clone(),
        access,
        provider,
    }
}

pub(in crate::hosted) fn rebuild(
    policy: &IngressOwnershipPolicy,
    original: &HostedContextRefresh,
    sources: ContextSources,
    tools: &[serde_json::Value],
) -> HostedContextRefresh {
    let home = format!(
        "/home/{}",
        original.0.path.strip_suffix("/AGENTS.md").unwrap()
    );
    HostedContextRefresh::new_admitted(
        policy,
        sources,
        ContextInput {
            handle: &original.0.handle,
            identity: &original.0.identity,
            home: &home,
            cwd: &original.0.cwd,
            tools,
        },
    )
    .unwrap()
}

pub(in crate::hosted) fn mutex_backing() -> usize {
    if cfg!(target_vendor = "apple") {
        size_of::<libc::pthread_mutex_t>()
    } else {
        0
    }
}
