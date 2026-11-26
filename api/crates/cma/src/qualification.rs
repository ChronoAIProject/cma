//! Explicit startup for the feature-enabled local qualification daemon.

use cma_qualification::{
    transport::{validate_parent, ControlServer, Plan},
    Registry,
};
use layer2_core::agent_ownership::OwnershipCoordinator;
use std::{path::PathBuf, sync::Arc};

const PLAN: &str = "--qualification-plan";
const SOCKET: &str = "--qualification-control-socket";
const USAGE: &str = "usage: cma --qualification-plan PLAN --qualification-control-socket SOCKET";

pub(crate) struct Options {
    plan: Plan,
    socket: PathBuf,
}

pub(crate) fn startup_options(arguments: &[String]) -> Option<Result<Options, &'static str>> {
    if !arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), PLAN | SOCKET))
    {
        return None;
    }
    Some(parse(arguments))
}

fn parse(arguments: &[String]) -> Result<Options, &'static str> {
    let [first, first_value, second, second_value] = arguments else {
        return Err(USAGE);
    };
    let (plan, socket) = match (first.as_str(), second.as_str()) {
        (PLAN, SOCKET) => (first_value, second_value),
        (SOCKET, PLAN) => (second_value, first_value),
        _ => return Err(USAGE),
    };
    let plan = PathBuf::from(plan);
    let socket = PathBuf::from(socket);
    if !plan.is_absolute() || !socket.is_absolute() {
        return Err(USAGE);
    }
    validate_parent(&socket).map_err(|_| "qualification socket directory refused")?;
    let build = option_env!("CHRONO_BUILD_SHA").ok_or("qualification build identity missing")?;
    let plan = Plan::open(&plan, build).map_err(|_| "qualification plan refused")?;
    Ok(Options { plan, socket })
}

impl Options {
    pub(crate) async fn bind(
        self,
        registry: Registry,
        coordinator: Option<Arc<OwnershipCoordinator>>,
        config: &crate::config::Config,
    ) -> Result<ControlServer, String> {
        let renewal_store = if self.plan.needs_renewal_store() {
            let coordinator = coordinator
                .as_ref()
                .ok_or("qualification renewal requires Agent ownership")?;
            let isolated =
                crate::store::agent_ownership::MongoAgentOwnership::prepare_qualification_renewal(
                    &config.mongodb_uri,
                    &config.mongodb_database,
                )
                .await?;
            if !coordinator.attach_qualification_renewal(Arc::new(registry.clone())) {
                return Err("qualification renewal hook already installed".into());
            }
            Some(isolated)
        } else {
            None
        };
        ControlServer::bind(self.socket, self.plan, registry, coordinator, renewal_store)
            .map_err(|_| "qualification control socket refused".into())
    }
}
