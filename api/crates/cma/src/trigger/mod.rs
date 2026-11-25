mod composition;
mod connections_nyxid;
mod inline;

#[cfg(test)]
mod connections_nyxid_tests;
#[cfg(test)]
pub(crate) use connections_nyxid_tests::GithubRepositoryFixture;

#[allow(
    dead_code,
    reason = "composed with the production Trigger routes introduced by #824"
)]
mod policy;
mod scheduler;
mod worker;

#[cfg(test)]
mod worker_tests;

#[allow(
    unused_imports,
    reason = "exported for the production Trigger composition introduced by #824"
)]
pub(crate) use self::{
    connections_nyxid::{CmaServiceConnectionBroker, VerifiedGitHubConnection},
    policy::LiveLaunchPolicy,
};
pub(crate) use composition::{TriggerComposition, TriggerCompositionDependencies};
pub(crate) use connections_nyxid::{
    GitHubConnections, GitHubRepositoryCheck, GitHubRepositoryNeed, GitHubScopeCheck,
    GITHUB_SERVICE,
};
pub(crate) use inline::InlineTriggerProfileMaterializer;
pub(crate) use scheduler::LaunchScheduler;

#[cfg(test)]
pub(crate) use worker::{
    LaunchResourceDriver, LaunchResourceEffect, LaunchResourceError, LaunchWorker,
    LaunchWorkerConfig, LaunchWorkerError, ServiceLaunchResourceDependencies,
    ServiceLaunchResourceDriver,
};
