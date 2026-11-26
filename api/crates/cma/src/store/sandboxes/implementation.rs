use super::*;
use crate::store::agent_log_integrity::IntegrityFailurePolicy;
use layer2_core::sandbox::RetainedReopenPath;

#[async_trait::async_trait]
impl SandboxStore for MongoSandboxStore {
    async fn owner_capacity(
        &self,
        owner: &SubjectId,
    ) -> Result<layer2_core::sandbox::SandboxCapacitySnapshot, SandboxStoreError> {
        super::capacity::read(self, owner).await
    }

    async fn failed_lifecycle_admission(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<layer2_core::sandbox::SandboxLifecycleAdmission>, SandboxStoreError> {
        super::failed_admission::read(self, sandbox).await
    }

    async fn retirement_admitted_at(
        &self,
        id: &SandboxId,
    ) -> Result<Option<String>, SandboxStoreError> {
        let document = self.sandbox_events.find_one(doc! {
            "sandbox_id": id.as_str(), "record.kind.type": "status_changed", "record.kind.to": "retiring",
        }).sort(doc! {"seq": 1}).max_time(std::time::Duration::from_secs(2)).await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let event = document
            .as_ref()
            .map(super::super::sandbox_events::read_event)
            .transpose()?;
        let Some(event) = event else { return Ok(None) };
        let admitted =
            super::super::sandbox_controls::retirement_admission_at(self, id, &event.appended_at)
                .await?;
        Ok(Some(admitted.unwrap_or(event.appended_at)))
    }
    async fn retained_reopen_snapshot(
        &self,
        id: &SandboxId,
    ) -> Result<layer2_core::sandbox::RetainedReopenSnapshot, SandboxStoreError> {
        super::super::retained_reopen::snapshot(
            self,
            id,
            RetainedReopenPath::Idle,
            IntegrityFailurePolicy::Quarantine,
        )
        .await
    }

    async fn retained_reopen_assessment_snapshot(
        &self,
        id: &SandboxId,
        path: RetainedReopenPath,
    ) -> Result<layer2_core::sandbox::RetainedReopenSnapshot, SandboxStoreError> {
        super::super::retained_reopen::snapshot(self, id, path, IntegrityFailurePolicy::ReportOnly)
            .await
    }

    async fn retained_reopen_receipt(
        &self,
        id: &SandboxId,
    ) -> Result<Option<layer2_core::sandbox::RetainedReopenReceipt>, SandboxStoreError> {
        super::super::retained_reopen::receipt(self, id).await
    }

    async fn retained_reopen_replay(
        &self,
        id: &SandboxId,
    ) -> Result<Option<layer2_core::sandbox::RetainedReopenReplay>, SandboxStoreError> {
        super::super::retained_reopen::replay(self, id).await
    }

    async fn retained_reopen_history(
        &self,
        id: &SandboxId,
        key_sha256: Option<&str>,
    ) -> Result<layer2_core::sandbox::RetainedReopenHistory, SandboxStoreError> {
        super::super::retained_reopen::read_for_service(self, id, key_sha256).await
    }

    async fn retained_suspended_reopen_snapshot(
        &self,
        id: &SandboxId,
    ) -> Result<layer2_core::sandbox::RetainedReopenSnapshot, SandboxStoreError> {
        super::super::retained_reopen::snapshot(
            self,
            id,
            RetainedReopenPath::Suspended,
            IntegrityFailurePolicy::Quarantine,
        )
        .await
    }

    async fn admit_retained_reopen(
        &self,
        request: layer2_core::sandbox::RetainedReopenRequest,
    ) -> Result<layer2_core::sandbox::RetainedReopenAccepted, SandboxStoreError> {
        super::super::retained_reopen::admit(self, request).await
    }

    async fn admit_suspended_reopen(
        &self,
        request: layer2_core::sandbox::RetainedSuspendedReopenRequest,
    ) -> Result<layer2_core::sandbox::RetainedReopenAccepted, SandboxStoreError> {
        super::super::retained_reopen::admit_suspended(self, request).await
    }

    async fn retained_runtime_recovery_snapshot(
        &self,
        id: &SandboxId,
    ) -> Result<layer2_core::sandbox::RetainedRecoverySnapshot, SandboxStoreError> {
        super::super::sandbox_unpublished_recovery::retained_snapshot(self, id).await
    }

    fn managed_runtime_version(&self) -> Option<String> {
        self.managed_agent_release()
    }

    fn desktop_runtime_target_is_qualified(&self, target: &str) -> bool {
        self.desktop_verified_runtime_image.as_deref() == Some(target)
    }

    fn machine_runtime_target_is_qualified(&self, id: &SandboxId, target: &str) -> bool {
        self.managed_machine_updates.includes(id)
            && crate::agent_lifecycle::valid_immutable_image(target)
    }

    fn managed_runtime_target_is_held(&self, target: &str) -> bool {
        self.default_pool_holds.image(target)
    }

    async fn qualify_retained_machine(
        &self,
        sandbox: &Sandbox,
        at: &str,
    ) -> Result<Option<Sandbox>, SandboxStoreError> {
        crate::store::managed_machine_origin::qualify(self, sandbox, at).await
    }

    async fn unpublished_runtime_recovery_snapshot(
        &self,
        id: &SandboxId,
    ) -> Result<layer2_core::sandbox::SandboxUnpublishedRecoverySnapshot, SandboxStoreError> {
        super::super::sandbox_unpublished_recovery::snapshot(self, id).await
    }

    async fn admit_unpublished_runtime_recovery(
        &self,
        request: layer2_core::sandbox::SandboxUnpublishedRecoveryRequest,
    ) -> Result<Sandbox, SandboxStoreError> {
        super::super::sandbox_unpublished_recovery::admit(self, request).await
    }

    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        if sandbox.lifecycle.runtime_image_origin.is_some() {
            return Err(SandboxStoreError::InvalidTransaction(
                "runtime image origin requires lifecycle admission",
            ));
        }
        if sandbox.lifecycle.custom_runtime.is_some()
            || sandbox.lifecycle.runtime_migration_admission.is_some()
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "custom runtime authority requires retained migration admission",
            ));
        }
        if self.global_sandbox_limit != 0 {
            return Err(SandboxStoreError::InvalidTransaction(
                "global capacity requires transactional sandbox admission",
            ));
        }
        if !sandbox.credential.service_grants.is_empty()
            || sandbox.credential.service_scope.is_some()
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "service access requires an existing sandbox and an atomic grant",
            ));
        }
        if sandbox.revision != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        let document = sandbox_document(&sandbox, 0)?;
        self.sandboxes
            .insert_one(document)
            .await
            .map(|_| ())
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    SandboxStoreError::Duplicate
                } else {
                    SandboxStoreError::Unavailable(error.to_string())
                }
            })
    }

    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        self.sandboxes
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_sandbox)
            .transpose()
    }

    async fn save_sandbox(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
    ) -> Result<(), SandboxStoreError> {
        self.save_sandbox_validated(expected_revision, sandbox, None)
            .await
    }

    fn capture_released_runtime(&self) -> Option<layer3_adapter::ReleasedRuntime> {
        let release = self.released_runtime();
        (!release.agent.is_empty()).then_some(release)
    }

    async fn refresh_released_runtime(&self) {
        if let Some(source) = &self.released {
            source.refresh_after_stale().await;
        }
    }

    async fn reserve_initial_runtime_image(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        image: &str,
    ) -> Result<String, SandboxStoreError> {
        super::super::released_runtimes::reserve_initial_runtime_image(
            self, sandbox, generation, image,
        )
        .await
    }

    async fn save_sandbox_against_release(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
        release: &layer3_adapter::ReleasedRuntime,
        lane: layer2_core::released_runtime::ReleasedLane,
    ) -> Result<(), SandboxStoreError> {
        self.save_sandbox_inner(expected_revision, sandbox, None, Some((release, lane)))
            .await
    }

    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.sandbox_page(doc! { "owner": owner.as_str() }, page)
            .await
    }

    async fn list_live_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.sandbox_page(
            doc! {
                "owner": owner.as_str(),
                "status": { "$nin": ["retiring", "retired"] },
            },
            page,
        )
        .await
    }
}

impl MongoSandboxStore {
    pub(in crate::store) async fn save_sandbox_validated(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
        acquisition: Option<&Document>,
    ) -> Result<(), SandboxStoreError> {
        self.save_sandbox_inner(expected_revision, sandbox, acquisition, None)
            .await
    }

    async fn save_sandbox_inner(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
        acquisition: Option<&Document>,
        release: Option<(
            &layer3_adapter::ReleasedRuntime,
            layer2_core::released_runtime::ReleasedLane,
        )>,
    ) -> Result<(), SandboxStoreError> {
        let next = expected_revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        if sandbox.revision != next {
            return Err(SandboxStoreError::Conflict);
        }
        let current = self
            .get_sandbox(&sandbox.id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if current.revision != expected_revision
            || current.lifecycle.custom_runtime != sandbox.lifecycle.custom_runtime
            || current.lifecycle.runtime_migration_admission
                != sandbox.lifecycle.runtime_migration_admission
        {
            return Err(SandboxStoreError::Conflict);
        }
        current
            .validate_retirement_archive_successor(&sandbox)
            .map_err(|_| SandboxStoreError::Conflict)?;
        current
            .validate_service_access_successor(&sandbox)
            .map_err(|_| SandboxStoreError::Conflict)?;
        current
            .validate_unpublished_recovery_save(&sandbox)
            .map_err(|_| SandboxStoreError::Conflict)?;
        current
            .validate_checkpoint_capture_successor(&sandbox)
            .map_err(|_| SandboxStoreError::Conflict)?;
        layer2_core::sandbox::ProvisioningExecution::validate_successor(
            current.work.execution,
            sandbox.work.execution,
        )
        .map_err(|_| SandboxStoreError::Conflict)?;
        current
            .validate_runtime_generation_successor(&sandbox)
            .map_err(|_| SandboxStoreError::Conflict)?;
        // Even a metadata-only revision must advance the usage watermark;
        // otherwise the next lifecycle edge looks like an unmetered writer gap.
        super::super::sandbox_usage::save::save(
            self,
            expected_revision,
            &sandbox,
            acquisition,
            release,
        )
        .await
    }

    async fn sandbox_page(
        &self,
        mut filter: Document,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .sandboxes
            .find(crate::store::work_schema::query(filter))
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            items.push(read_sandbox(&document)?);
        }
        let has_more = items.len() > usize::from(page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("a bounded page has a last item")
                .id
                .0
                .clone()
        });
        Ok(Page { items, next })
    }
}
