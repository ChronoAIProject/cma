//! Offline archive export. No supervisor, Agent, credential or network service starts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use layer3_adapter::{
    AgentSlot, CheckpointReceipt, HomeSnapshotExportConversation, HomeSnapshotExportReceipt,
    HomeSnapshotExportRequest, WriterState, MAX_SNAPSHOT_EXPORT_CONVERSATIONS,
};

use crate::config::RuntimeConfig;
use crate::failure::Result;

#[path = "snapshot_export_io.rs"]
mod io;
use io::invalid;

pub(crate) async fn run(arguments: &[std::ffi::OsString]) -> Result<()> {
    if arguments.len() != 2 {
        return Err(invalid(
            "snapshot export requires a request file and an existing output directory",
        ));
    }
    let request_path = PathBuf::from(&arguments[0]);
    let output = PathBuf::from(&arguments[1]);
    let request: HomeSnapshotExportRequest =
        io::read(&request_path)?.ok_or_else(|| invalid("snapshot export request is missing"))?;
    request
        .validate()
        .map_err(|_| invalid("snapshot export request is invalid"))?;
    let config = source_config(&request, &output)?;
    let source = ReadOnlySource::open(config, &output)?;
    let receipt = tokio::task::spawn_blocking(move || export(&source, &request, &output))
        .await
        .map_err(|_| invalid("snapshot export task did not complete"))??;
    tracing::info!(
        archive_bytes = receipt.archive_bytes,
        conversations = receipt.conversations.len(),
        "immutable home export completed"
    );
    Ok(())
}

fn source_config(request: &HomeSnapshotExportRequest, output: &Path) -> Result<RuntimeConfig> {
    let sandbox_id = &request.snapshot.home.request.sandbox_id;
    crate::paths::safe_component(sandbox_id, "snapshot sandbox id")?;
    Ok(RuntimeConfig {
        bind: "127.0.0.1:0".parse().expect("literal loopback address"),
        sandbox_id: sandbox_id.clone(),
        generation: request.runtime_generation,
        home_identity: Some(
            request
                .snapshot
                .home
                .identity()
                .map_err(|_| invalid("snapshot home identity is invalid"))?,
        ),
        home: PathBuf::from("/home/cma"),
        state_root: PathBuf::from("/var/lib/cma-runtime").join(sandbox_id),
        checkpoint_root: output.to_path_buf(),
        install_env: BTreeMap::new(),
        process_env: BTreeMap::new(),
        model_proxy: None,
        platform_proxy: None,
        github_proxy: None,
        nyxid_url: None,
        allow_local_git: false,
    })
}

struct ReadOnlySource {
    config: RuntimeConfig,
}

impl ReadOnlySource {
    fn open(config: RuntimeConfig, output: &Path) -> Result<Self> {
        for path in [&config.home, &config.state_root, output] {
            io::directory(path)?;
        }
        if output.starts_with(&config.home)
            || config.home.starts_with(output)
            || output.starts_with(&config.state_root)
            || config.state_root.starts_with(output)
            || !io::read_only(&config.home)?
            || !io::read_only(&config.state_root)?
            || io::read_only(output)?
        {
            return Err(invalid("snapshot home and journals must be mounted read-only with separate writable export storage"));
        }
        Ok(Self { config })
    }

    fn verify(&self, request: &HomeSnapshotExportRequest) -> Result<()> {
        if self.config.sandbox_id != request.snapshot.home.request.sandbox_id
            || self.config.generation != request.runtime_generation
            || self.config.home_identity.as_deref()
                != Some(
                    request
                        .snapshot
                        .home
                        .identity()
                        .map_err(|_| invalid("snapshot home identity is invalid"))?
                        .as_str(),
                )
        {
            return Err(invalid("snapshot export source does not match its request"));
        }
        let binding = crate::persistent_home::verify(&self.config)?
            .ok_or_else(|| invalid("snapshot source has no persistent binding"))?;
        if !binding.installation_ready(crate::install_state::inspect(&self.config)?.as_ref())? {
            return Err(invalid("snapshot source has no completed installation"));
        }
        Ok(())
    }
}

fn export(
    source: &ReadOnlySource,
    request: &HomeSnapshotExportRequest,
    output: &Path,
) -> Result<HomeSnapshotExportReceipt> {
    request
        .validate()
        .map_err(|_| invalid("snapshot export request is invalid"))?;
    source.verify(request)?;
    io::publish(&output.join("export-request.json"), request)?;
    let receipt_path = output.join("export.json");
    if let Some(receipt) = io::read::<HomeSnapshotExportReceipt>(&receipt_path)? {
        receipt
            .validate_for(request)
            .map_err(|_| invalid("retained export receipt is inconsistent"))?;
        verify_archive(output, &receipt)?;
        return Ok(receipt);
    }
    let slots = agent_slots(&source.config.home)?;
    let exclusions = layer3_adapter::codex::checkpoint_exclusions(&slots)
        .map_err(|_| invalid("snapshot export exclusions are invalid"))?;
    let mut conversations = Vec::new();
    for identity in &request.conversations {
        let home = source.config.agent_home(&identity.agent_id);
        if !crate::paths::path_is_directory(&home) {
            return Err(invalid("snapshot is missing an admitted agent home"));
        }
        let captured = crate::conversation::capture_named(&home, &identity.conversation_id)?;
        conversations.push(HomeSnapshotExportConversation {
            identity: identity.clone(),
            archive_sha256: captured.archive_sha256,
        });
    }
    let archive = crate::checkpoint_archive::seal(&source.config.home, output, &exclusions)?;
    source.verify(request)?;
    let receipt = HomeSnapshotExportReceipt {
        request_sha256: request
            .computed_sha256()
            .map_err(|_| invalid("snapshot export request is invalid"))?,
        checkpoint: CheckpointReceipt {
            epoch: request.snapshot.epoch,
            object_reference: archive.reference,
            sha256: archive.sha256,
            writers: WriterState::Resumed,
        },
        archive_bytes: archive
            .path
            .metadata()
            .map_err(|_| invalid("export archive is missing"))?
            .len(),
        conversations,
    };
    receipt
        .validate_for(request)
        .map_err(|_| invalid("snapshot export receipt is invalid"))?;
    io::publish(&receipt_path, &receipt)?;
    Ok(receipt)
}

fn verify_archive(output: &Path, receipt: &HomeSnapshotExportReceipt) -> Result<()> {
    let path = output.join(&receipt.checkpoint.object_reference);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| invalid("retained export archive is missing"))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != receipt.archive_bytes
        || crate::checkpoint_manifest::sha256_file(&path)? != receipt.checkpoint.sha256
    {
        return Err(invalid(
            "retained export archive does not match its receipt",
        ));
    }
    Ok(())
}

fn agent_slots(home: &Path) -> Result<Vec<AgentSlot>> {
    for path in [home.join(".cma"), home.join(".cma/agents")] {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
            _ => return Err(invalid("snapshot agent parent is not a real directory")),
        }
    }
    let mut slots = Vec::new();
    for entry in std::fs::read_dir(home.join(".cma/agents"))
        .map_err(|_| invalid("snapshot agents cannot be enumerated"))?
    {
        let entry = entry.map_err(|_| invalid("snapshot agent entry is unavailable"))?;
        let id = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("snapshot agent name is invalid"))?;
        crate::paths::safe_component(&id, "snapshot agent id")?;
        if !entry
            .file_type()
            .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            || slots.len() >= MAX_SNAPSHOT_EXPORT_CONVERSATIONS
        {
            return Err(invalid(
                "snapshot agent directory is invalid or exceeds its bound",
            ));
        }
        slots.push(AgentSlot::new(id));
    }
    Ok(slots)
}

#[cfg(test)]
#[path = "snapshot_export_tests.rs"]
mod tests;
