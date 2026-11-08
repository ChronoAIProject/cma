//! Format selection precedes installation recovery, desktop launch and serving.
use super::*;

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
        let mut prepared = prepare(&config, true)?;
        let ledger = prepared
            .selection
            .take()
            .expect("production format selection")
            .open(&config)
            .await?;
        let supervisor = Self::from_prepared(config, writer_boundary, prepared, ledger)?;
        if supervisor.config.machine_included {
            let ready = async {
                loop {
                    if tokio::net::TcpStream::connect("127.0.0.1:6080")
                        .await
                        .is_ok()
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            tokio::time::timeout(std::time::Duration::from_secs(30), ready)
                .await
                .map_err(|_| {
                    RuntimeError::unavailable(
                        "desktop_start_failed",
                        "desktop did not become ready",
                    )
                })?;
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
        Self::from_prepared(config, writer_boundary, prepared, None)
    }

    fn from_prepared(
        config: RuntimeConfig,
        writer_boundary: SupervisorBoundary,
        prepared: Prepared,
        ledger: Option<Arc<crate::mutation_ledger::Ledger>>,
    ) -> Result<Arc<Self>> {
        let Prepared {
            boot_id,
            home,
            _writer,
            ..
        } = prepared;
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
            Some(crate::desktop_process::DesktopProcess::start(&config)?)
        } else {
            None
        };
        Ok(Arc::new(Self {
            mutations: crate::mutation_store::Store::new(config.clone(), ledger),
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
