//! Format selection precedes installation recovery, desktop launch and serving.
use std::{future::Future, time::Duration};

use super::*;

const DEFAULT_DESKTOP_STARTUP_BUDGET: Duration = Duration::from_secs(30);
const EXTENDED_DESKTOP_STARTUP_BUDGET: Duration = Duration::from_secs(90);
const DESKTOP_PROBE_INTERVAL: Duration = Duration::from_millis(100);

struct Prepared {
    boot_id: String,
    home: Option<crate::persistent_home::Binding>,
    _writer: Option<std::fs::File>,
    selection: Option<crate::ledger_format::Selection>,
}

fn prepare(config: &RuntimeConfig, indexed: bool) -> Result<Prepared> {
    if config.home_identity.is_none() {
        crate::fs_durability::create_dir_all(&config.home).map_err(|error| {
            RuntimeError::unavailable("home_unavailable", format!("create sandbox home: {error}"))
        })?;
    }
    if !path_is_directory(&config.home) {
        return Err(RuntimeError::rejected(
            "home_invalid",
            "sandbox home is not a directory",
        ));
    }
    let boot_id = uuid::Uuid::new_v4().simple().to_string();
    let _removal_writer = crate::workspace_removal_writer::claim(config, &boot_id)?;
    let selection = if indexed {
        Some(crate::ledger_format::select(config)?)
    } else {
        None
    };
    let home = crate::persistent_home::initialize(config)?;
    crate::workspace_removal_writer::publish_claim(config, &boot_id)?;
    Ok(Prepared {
        boot_id,
        home,
        _writer: _removal_writer,
        selection,
    })
}

impl Supervisor {
    pub(crate) async fn start(
        config: RuntimeConfig,
        writer_boundary: SupervisorBoundary,
    ) -> Result<Arc<Self>> {
        // The archive is a required admission dependency once activated. Open
        // it before even home-generation publication or installation recovery.
        let retirement = crate::retirement_format::select(&config)?
            .open(&config)
            .await?;
        let mut prepared = prepare(&config, true)?;
        let ledger = prepared
            .selection
            .take()
            .expect("production format selection")
            .open(&config)
            .await?;
        let supervisor =
            Self::from_prepared(config, writer_boundary, prepared, ledger, retirement)?;
        Self::finish_startup(supervisor).await
    }

    pub(super) async fn finish_startup(supervisor: Arc<Self>) -> Result<Arc<Self>> {
        Self::finish_startup_with_probe(supervisor, || async {
            tokio::net::TcpStream::connect("127.0.0.1:6080")
                .await
                .is_ok()
        })
        .await
    }

    async fn finish_startup_with_probe<Fut: Future<Output = bool>>(
        supervisor: Arc<Self>,
        mut connect: impl FnMut() -> Fut,
    ) -> Result<Arc<Self>> {
        if supervisor.config.machine_included {
            let budget = if supervisor.config.extended_desktop_startup {
                EXTENDED_DESKTOP_STARTUP_BUDGET
            } else {
                DEFAULT_DESKTOP_STARTUP_BUDGET
            };
            let started = tokio::time::Instant::now();
            let ready = match &supervisor.desktop_process {
                #[cfg(target_os = "linux")]
                Some(crate::desktop_process::DesktopSession::Supervised(desktop)) => {
                    desktop.ready(budget).await
                }
                _ => {
                    let ready = async {
                        while !connect().await {
                            tokio::time::sleep(DESKTOP_PROBE_INTERVAL).await;
                        }
                    };
                    tokio::select! {
                        biased;
                        _ = tokio::time::sleep(budget) => false,
                        _ = ready => true,
                    }
                }
            };
            if !ready {
                tracing::error!(
                    elapsed_ms = started.elapsed().as_millis(),
                    "initial desktop did not become ready"
                );
                if let Some(desktop) = &supervisor.desktop_process {
                    desktop.shutdown().await;
                }
                return Err(RuntimeError::unavailable(
                    "desktop_start_failed",
                    "desktop did not become ready",
                ));
            }
            tracing::info!(
                elapsed_ms = started.elapsed().as_millis(),
                "initial desktop ready"
            );
        }
        supervisor.recover_journals().await?;
        Ok(supervisor)
    }

    // Existing V1 fixtures deliberately keep their original format.
    #[cfg(test)]
    pub(crate) fn new(
        config: RuntimeConfig,
        writer_boundary: SupervisorBoundary,
    ) -> Result<Arc<Self>> {
        let prepared = prepare(&config, false)?;
        crate::retirement_format::require_absent(&config)?;
        Self::from_prepared(config, writer_boundary, prepared, None, None)
    }

    fn from_prepared(
        config: RuntimeConfig,
        writer_boundary: SupervisorBoundary,
        prepared: Prepared,
        ledger: Option<Arc<crate::mutation_ledger::Ledger>>,
        retirement: Option<crate::retirement_format::Reader>,
    ) -> Result<Arc<Self>> {
        let Prepared {
            boot_id,
            home,
            _writer,
            ..
        } = prepared;
        match &retirement {
            Some(reader) => reader.verify()?,
            None => crate::retirement_format::require_absent(&config)?,
        }
        let install_state = crate::install::recover(&config)?;
        let restored = home
            .as_ref()
            .map(|home| home.restore_completed(&install_state))
            .transpose()?
            .unwrap_or(false);
        let (phase, restore_bootstrap) = match install_state {
            RecoveryState::Completed => (Phase::Installed, None),
            RecoveryState::Inherited(receipt) => (
                if restored {
                    Phase::Installed
                } else {
                    Phase::RestorePending
                },
                Some(receipt),
            ),
            RecoveryState::Absent | RecoveryState::Terminal => (Phase::Provisioned, None),
        };
        let checkpoint = CheckpointStore::new(&config);
        let desktop_process = if config.machine_included {
            Some(crate::desktop_process::DesktopSession::start(&config)?)
        } else {
            None
        };
        Ok(Arc::new(Self {
            desktop_ownership: crate::desktop_ownership::DesktopReceiver::new(
                config.state_root.join("desktop-owner"),
                crate::desktop_ownership::Identity {
                    sandbox: config.sandbox_id.clone(),
                    generation: config.generation,
                    home: config.home_identity.clone(),
                    boot: boot_id.clone(),
                },
            ),
            manager_ownership: crate::manager_ownership::ManagerOwnership::new(
                config.state_root.join("agent-manager-owners"),
            ),
            mutations: crate::mutation_store::Store::new(config.clone(), ledger)
                .with_retirement(retirement),
            config,
            boot_id,
            phase: RwLock::new(phase),
            allow_unowned_preparation: phase == Phase::Provisioned,
            restore_bootstrap: RwLock::new(restore_bootstrap),
            operation_gate: Arc::new(Mutex::new(())),
            credential: CredentialHolder::new(),
            agents: RwLock::new(BTreeMap::new()),
            browsers: Mutex::new(BTreeMap::new()),
            checkpoint,
            held_checkpoint: Arc::new(Mutex::new(None)),
            resources: crate::resources::Sampler::new(
                crate::resources::Roots::default(),
                std::time::Instant::now(),
            ),
            writer_boundary,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("skill HTTP client builds"),
            desktop_process,
        }))
    }
}

#[cfg(test)]
#[path = "supervisor_startup_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "home_restore_preflight_tests.rs"]
mod home_restore_preflight_tests;
