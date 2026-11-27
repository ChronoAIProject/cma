use super::*;
use crate::agent_sandbox::pool::{fixture::prefix, read_fixture::Effects};
use crate::{AgentFileReadReceipt, AgentFileReadRequest};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};

#[derive(Default)]
struct FileTransport(AtomicUsize);

#[async_trait::async_trait]
impl HostedControlTransport for FileTransport {
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("checked read must use the bound exchange");
    }

    async fn exchange_bound_file_read(
        &self,
        _: &RuntimeHandle,
        _: &crate::agent_sandbox::PodBinding,
        _: &str,
        request: AgentFileReadRequest,
    ) -> Result<Option<ResponseFrame>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        assert!(request.home_identity.is_some());
        Ok(Some(ResponseFrame::ReadAgentFile(AgentFileReadReceipt {
            path: request.path,
            size_bytes: 3,
            sha256: format!("{:x}", Sha256::digest(b"CMA")),
            content_base64: "Q01B".into(),
        })))
    }

    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("file read must not open an Agent");
    }

    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        Ok(())
    }
}

struct FileFixture {
    bound: Bound,
    home: HomeVolumeAttachment,
    backend: AgentSandboxBackend,
    transport: Arc<FileTransport>,
    effects: Effects,
}

impl FileFixture {
    async fn start() -> Self {
        let bound = bound(true).await;
        let home = adopted(&bound.fixture, &bound.claim).await;
        let effects = Effects::new(&bound.fixture, BTreeMap::new());
        let client = AgentSandboxClient::new(
            effects.client.clone(),
            bound.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap();
        let transport = Arc::new(FileTransport::default());
        let backend = AgentSandboxBackend::new(client, transport.clone())
            .with_single_exchange_file_reads(true);
        assert_eq!(
            backend.provision(bound.request.clone()).await.unwrap(),
            bound.runtime
        );
        effects.calls.lock().unwrap().clear();
        Self {
            bound,
            home,
            backend,
            transport,
            effects,
        }
    }

    async fn read(&self) -> Result<Option<AgentFileReadReceipt>> {
        let identity = crate::hosted::provider_request_identity(&self.bound.request).unwrap();
        self.backend
            .read_file_with_binding(
                &self.bound.runtime,
                &self.home,
                &AgentFileReadRequest {
                    owner_access: true,
                    protected_paths: vec![],
                    sandbox_id: identity.sandbox_id,
                    runtime_generation: identity.generation,
                    boot_id: "a".repeat(32),
                    home_identity: None,
                    path: "cma/file.txt".into(),
                    max_bytes: 3,
                },
            )
            .await
    }

    fn template(&self) -> String {
        format!(
            "{}/sandboxtemplates/{}",
            prefix(),
            self.bound.claim.request.pool.template_name
        )
    }
}

#[tokio::test]
async fn checked_file_read_reuses_home_proof_and_refuses_changed_live_template() {
    let fixture = FileFixture::start().await;
    let template = fixture.template();
    assert_eq!(
        fixture.read().await.unwrap().unwrap().content().unwrap(),
        b"CMA"
    );
    fixture.effects.calls.lock().unwrap().clear();

    assert!(fixture.read().await.unwrap().is_some());
    let reads = fixture.effects.calls.lock().unwrap().clone();
    assert!(reads.contains(&format!("GET {template}")));
    assert!(!reads.contains(&format!("GET {VOLUME}")));

    fixture
        .bound
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(&template)
        .unwrap()["metadata"]["uid"] = json!("changed-template");
    assert!(matches!(
        fixture.read().await,
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn checked_file_read_with_slow_binding_proof_does_not_dispatch() {
    use crate::agent_sandbox::pool::read_fixture::Gate;

    let fixture = FileFixture::start().await;
    let gate = Gate::new();
    fixture
        .effects
        .arm_read_gates(BTreeMap::from([(VOLUME.into(), gate.clone())]));
    tokio::time::pause();
    let release = async {
        gate.wait().await;
        tokio::time::advance(Duration::from_secs(9)).await;
        gate.respond();
    };
    let (result, ()) = tokio::join!(fixture.read(), release);
    assert!(
        matches!(result, Err(AdapterError::Unavailable(error)) if error == "file read binding expired")
    );
    assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 0);
    assert!(fixture.read().await.unwrap().is_some());
    assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 1);
}
