//! Ways of assembling a service for a test.
//!
//! Split from `test_support` so both stay within the project's file-size
//! limit, on the division that was already there: that file says what stands
//! in for a dependency, and this says how the service under test is wired out
//! of those stand-ins.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::KernelAdapter;

use crate::ids::KernelId;
use crate::service::{Deployment, ScreenFactory, SessionService};

use crate::ids::SessionId;
use crate::test_support::{
    kernel, no_environments, FakeAdapter, FakeStore, FixedCredential, NullScreen, RecordingScreen,
};

/// A service that stores transcripts, over a screen model that keeps them.
pub(crate) fn service_with_context(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    context: Arc<dyn crate::store::ContextStore>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|_| Box::new(RecordingScreen::default()));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        Some(context),
        None,
        no_environments(),
        owned(),
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A service that expects a particular runtime version.
///
/// Two of these over one store is an upgrade: the second expects a version the
/// first's sessions are not on.
pub(crate) fn service_on_version(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    version: &str,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|_| Box::new(RecordingScreen::default()));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        None,
        None,
        no_environments(),
        Deployment {
            runtime_version: Some(version.to_string()),
            ..owned()
        },
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A service that can supply an owner's credential without the owner present.
pub(crate) fn service_with_credentials(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    credential: &str,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        None,
        Some(Arc::new(FixedCredential(credential.to_string()))),
        no_environments(),
        owned(),
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// Put bytes on a session's screen, as its runtime would.
pub(crate) async fn write_screen(svc: &Arc<SessionService>, id: &SessionId, bytes: &[u8]) {
    let entries = svc.entries.read().await;
    let entry = entries.get(id).expect("the session is live");
    entry.screen.lock().await.apply(bytes);
}

/// The credential a live session is currently holding.
pub(crate) async fn credential_on(svc: &Arc<SessionService>, id: &SessionId) -> Option<String> {
    svc.entries
        .read()
        .await
        .get(id)
        .and_then(|e| e.subject_credential.clone())
}

/// A service identifying itself as a particular deployment.
pub(crate) fn service_as(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    deployment: Deployment,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        None,
        None,
        no_environments(),
        deployment,
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A deployment that owns its provider, which is what most tests are about.
pub(crate) fn owned() -> Deployment {
    Deployment {
        adopts_untagged: true,
        ..Deployment::default()
    }
}
