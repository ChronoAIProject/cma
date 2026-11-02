use super::*;

#[async_trait::async_trait]
impl SandboxStore for MongoSandboxStore {
    fn managed_runtime_version(&self) -> Option<&str> {
        self.managed_runtime_version.as_deref()
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
        if current.revision != expected_revision {
            return Err(SandboxStoreError::Conflict);
        }
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
        let expected = signed(expected_revision, "expected sandbox revision")
            .map_err(|_| SandboxStoreError::Conflict)?;
        let (origin_type, origin_launch_id, origin_operation_id) = sandbox_origin_fields(&sandbox)?;
        let result = self
            .sandboxes
            .update_one(
                doc! {
                    "_id": sandbox.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": expected,
                    "channel_reuse_cleanup_execution": { "$exists": false },
                    "channel_retirement_finalization": { "$exists": false },
                    "origin_type": origin_type,
                    "origin_launch_id": origin_launch_id,
                    "origin_operation_id": origin_operation_id,
                },
                doc! { "$set": sandbox_replacement_fields(&sandbox)? },
            )
            .await
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    SandboxStoreError::Duplicate
                } else {
                    SandboxStoreError::Unavailable(error.to_string())
                }
            })?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(classify_cas_miss(&self.sandboxes, &sandbox.id).await?)
        }
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
            .find(filter)
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
