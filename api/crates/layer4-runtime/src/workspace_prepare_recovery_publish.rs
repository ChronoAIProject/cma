use super::*;

pub(super) async fn prepare_and_publish(
    config: &RuntimeConfig,
    request: &WorkspacePrepareRecoveryRequest,
    paths: &Paths,
    attempt: &Path,
    journal_sha256: &str,
    source_commit: Option<&str>,
) -> Result<WorkspacePrepareRecoveryReceipt> {
    let mut staged_request = request.original.clone();
    staged_request
        .name
        .clone_from(&request.original.workspace_id);
    let publication_path = attempt.join("publication.json");
    let publication = if let Some(publication) = read::<Publication>(&publication_path)? {
        publication
    } else {
        require_absent(&paths.published)?;
        let candidates = paths.candidate_root.parent().unwrap();
        ensure_directory(candidates)?;
        ensure_directory(&paths.candidate_root)?;
        let started = attempt.join("started.json");
        let receipt = if read::<bool>(&started)?.is_some() {
            crate::workspace_source::inspect_prepared(
                &paths.candidate_root,
                config.allow_local_git,
                &staged_request,
            )
            .await?
            .ok_or_else(|| invalid("interrupted recovery candidate has no completion proof"))?
        } else {
            require_absent(&paths.candidate)?;
            append(&started, &true)?;
            if matches!(
                staged_request.source,
                layer3_adapter::WorkspaceSource::RepositorySnapshot { .. }
                    | layer3_adapter::WorkspaceSource::PublicRepositorySnapshot { .. }
            ) {
                crate::repository_snapshot::prepare_pinned(
                    config,
                    &paths.candidate_root,
                    &staged_request,
                    source_commit
                        .ok_or_else(|| invalid("original repository source pin is absent"))?,
                )
                .await?
            } else {
                crate::workspace_source::prepare(
                    &paths.candidate_root,
                    config.allow_local_git,
                    &staged_request,
                )
                .await?
            }
        };
        let publication = Publication {
            directory: directory(&paths.candidate)?
                .ok_or_else(|| invalid("prepared recovery candidate disappeared"))?,
            resolved_commit: receipt.resolved_commit,
            source_commit: receipt.source_commit,
        };
        if publication.source_commit.as_deref() != source_commit {
            return Err(invalid(
                "prepared recovery candidate changed original source pin",
            ));
        }
        append(&publication_path, &publication)?;
        publication
    };
    if publication.source_commit.as_deref() != source_commit {
        return Err(invalid("recovery publication changed original source pin"));
    }
    match (directory(&paths.candidate)?, directory(&paths.published)?) {
        (Some(candidate), None) if candidate == publication.directory => {
            let inspected = crate::workspace_source::inspect_prepared(
                &paths.candidate_root,
                config.allow_local_git,
                &staged_request,
            )
            .await?
            .ok_or_else(|| invalid("prepared recovery candidate disappeared"))?;
            if inspected.resolved_commit != publication.resolved_commit
                || inspected.source_commit != publication.source_commit
            {
                return Err(invalid("prepared recovery candidate changed commit"));
            }
            crate::fs_durability::rename(&paths.candidate, &paths.published).map_err(io_error)?;
        }
        (None, Some(published)) if published == publication.directory => {}
        _ => {
            return Err(invalid(
                "recovery publication does not match its exact prepared directory",
            ))
        }
    }
    let inspected = crate::workspace_source::inspect_prepared(
        &config.home,
        config.allow_local_git,
        &request.original,
    )
    .await?
    .ok_or_else(|| invalid("recovered Workspace publication disappeared"))?;
    if inspected.resolved_commit != publication.resolved_commit
        || inspected.source_commit != publication.source_commit
    {
        return Err(invalid(
            "recovered Workspace commit changed before completion",
        ));
    }
    Ok(WorkspacePrepareRecoveryReceipt {
        prepared: WorkspaceReceipt {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            workspace_id: request.original.workspace_id.clone(),
            name: request.original.name.clone(),
            path: paths.published.to_string_lossy().into_owned(),
            resolved_commit: publication.resolved_commit,
            source_commit: publication.source_commit,
        },
        preserved_path: paths.preserved.to_string_lossy().into_owned(),
        original_journal_sha256: journal_sha256.into(),
    })
}
