//! Controlled parent writes immediately before the Agent's publication snapshot.
use super::*;

pub(super) fn advance(callback: &Trace, observer: &MongoSandboxStore) {
    let transaction = callback.bodies.fetch_add(1, Ordering::SeqCst) + 1;
    let delivery = callback.deliver_initial_credential.lock().unwrap().take();
    if let Some(id) = delivery {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut sandbox = observer.get_sandbox(&id).await.unwrap().unwrap();
                let revision = sandbox.revision;
                assert_eq!(sandbox.work.step, SandboxWorkStep::DeliverInitialCredential);
                assert!(!sandbox.credential.delivered);
                sandbox.credential.current = sandbox.credential.pending.take();
                sandbox.credential.delivered = true;
                sandbox.work.step = SandboxWorkStep::CreateProfileWorkspace(0);
                sandbox.revision += 1;
                observer.save_sandbox(revision, sandbox).await.unwrap();
            });
        });
    }
    let ready = callback.publish_ready_workspace.lock().unwrap().take();
    if let Some(mut ready) = ready {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let current = observer.get_workspace(&ready.id).await.unwrap().unwrap();
                ready.revision = current.revision + 1;
                ready.next_worktree_number = current.next_worktree_number;
                observer
                    .save_workspace(current.revision, ready)
                    .await
                    .unwrap();
            });
        });
    }
    let advance = {
        let mut pending = callback.workspace_advance.lock().unwrap();
        pending.take_if(|advance| advance.transaction == transaction)
    };
    if let Some(advance) = advance {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut workspace = observer.get_workspace(&advance.id).await.unwrap().unwrap();
                let revision = workspace.revision;
                workspace.revision += 1;
                workspace.next_worktree_number += 1;
                observer.save_workspace(revision, workspace).await.unwrap();
            });
        });
    }
}
