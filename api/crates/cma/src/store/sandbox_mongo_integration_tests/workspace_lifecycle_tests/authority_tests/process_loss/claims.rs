use super::*;

pub(super) async fn captured(store: &MongoSandboxStore, kind: Kind) -> ClaimedWork {
    match kind {
        Kind::Sandbox => ClaimedWork::Sandbox(Box::new(
            store
                .get_sandbox(&SandboxId(kind.id().into()))
                .await
                .unwrap()
                .unwrap(),
        )),
        Kind::Workspace => ClaimedWork::Workspace(Box::new(
            store
                .get_workspace(&WorkspaceId(kind.id().into()))
                .await
                .unwrap()
                .unwrap(),
        )),
    }
}

pub(super) async fn renew(
    store: &MongoSandboxStore,
    claim: &ClaimedWork,
) -> Result<Option<WorkLeaseGrant>, SandboxStoreError> {
    match claim {
        ClaimedWork::Sandbox(row) => store.renew_sandbox_authority(row, &at(0), &at(15)).await,
        ClaimedWork::Workspace(row) => store.renew_workspace_authority(row, &at(0), &at(15)).await,
    }
}

pub(super) async fn complete(
    store: &MongoSandboxStore,
    claim: &ClaimedWork,
) -> Result<(), SandboxStoreError> {
    match claim {
        ClaimedWork::Sandbox(row) => {
            let mut next = row.as_ref().clone();
            next.name = "survivor-publication".into();
            next.work.schedule(SandboxWorkStep::None, at(0));
            next.revision += 1;
            store.save_sandbox(row.revision, next).await
        }
        ClaimedWork::Workspace(row) => {
            let mut next = row.as_ref().clone();
            next.name = "survivor-publication".into();
            next.work.schedule(WorkspaceWorkStep::None, at(0));
            next.revision += 1;
            store.save_workspace(row.revision, next).await
        }
    }
}
