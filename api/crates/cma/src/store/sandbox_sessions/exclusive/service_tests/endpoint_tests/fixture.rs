use super::*;
use crate::auth::desktop_test_support::DesktopAuthFixture;

#[path = "audit.rs"]
mod audit;

pub(super) struct Fixture {
    audit: audit::Audit,
    pub(super) resources: Services,
    pub(super) auth: DesktopAuthFixture,
    pub(super) ingresses: [Ingress; 2],
    pub(super) root: child::ReceiverRoot,
    pub(super) child: Option<child::ReceiverChild>,
    pub(super) proxy: provider::ProviderProxy,
}

impl Fixture {
    pub(super) async fn new(replicas: bool) -> Self {
        let mut audit = audit::Audit::start().await;
        let resources = Services::new(replicas).await;
        let auth = DesktopAuthFixture::new(resources.caller.subject.clone()).await;
        assert_eq!(auth.provider_snapshot(), Default::default());
        audit.bind(resources.stores.db.name(), auth.database_name());
        let root = child::ReceiverRoot::new();
        let sandbox = resources.stores.request.sandbox_id.as_str();
        let child = child::ReceiverChild::start(&root, sandbox).await;
        let proxy = provider::ProviderProxy::start(
            child.address,
            &resources.stores.request.runtime.runtime_id,
            root.token(),
        )
        .await;
        let left = super::super::fixture::service(
            resources.stores.left.clone(),
            proxy.adapter(sandbox).await,
            replicas,
        );
        let right = super::super::fixture::service(
            resources.stores.right.clone(),
            proxy.adapter(sandbox).await,
            replicas,
        );
        let mut ingresses = Vec::new();
        for (service, state) in [(left, auth.left.clone()), (right, auth.right.clone())] {
            let app = layer1_public::rest::desktop_router(Arc::new(service), ORIGIN.into()).layer(
                axum::middleware::from_fn_with_state(state, crate::auth::attach_subject),
            );
            ingresses.push(Ingress::start(app).await);
        }
        Self {
            audit,
            resources,
            auth,
            ingresses: ingresses.try_into().ok().unwrap(),
            root,
            child: Some(child),
            proxy,
        }
    }

    pub(super) async fn discovery(&self, ingress: usize) -> reqwest::Response {
        self.ingresses[ingress]
            .discovery(&self.resources.agents[ingress], &self.auth.headers)
            .await
    }

    pub(super) async fn connect(&self, ingress: usize) -> Result<Socket, Error> {
        let request = self.ingresses[ingress].websocket(
            &self.resources.agents[ingress],
            ORIGIN,
            &self.auth.headers,
        );
        let connected = tokio::time::timeout(
            Duration::from_secs(12),
            tokio_tungstenite::connect_async(request),
        )
        .await;
        match connected {
            Ok(result) => result.map(|(socket, _)| socket),
            Err(_) => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "public handshake deadline",
            ))),
        }
    }

    pub(super) async fn available(&self, ingress: usize, within: Duration) {
        tokio::time::timeout(within, async {
            loop {
                let response = self.discovery(ingress).await;
                match response.status().as_u16() {
                    200 => break,
                    409 => tokio::time::sleep(Duration::from_millis(25)).await,
                    status => panic!("unexpected discovery status: {status}"),
                }
            }
        })
        .await
        .expect("claim must eventually become available");
    }

    pub(super) async fn restart_receiver(&mut self) {
        self.child.take().unwrap().kill(&self.root).await;
        let child = child::ReceiverChild::start(
            &self.root,
            self.resources.stores.request.sandbox_id.as_str(),
        )
        .await;
        self.proxy.receiver(child.address);
        self.child = Some(child);
    }

    pub(super) fn raw_receiver_request(
        &self,
        owner: Option<&layer3_adapter::DesktopOwnership>,
    ) -> axum::http::Request<()> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!(
            "ws://{}/runtime/desktop",
            self.child.as_ref().unwrap().address
        )
        .into_client_request()
        .unwrap();
        let mut token: axum::http::HeaderValue = self.root.token().parse().unwrap();
        token.set_sensitive(true);
        request.headers_mut().insert("x-cma-runtime-token", token);
        if let Some(owner) = owner {
            request.headers_mut().insert(
                layer3_adapter::desktop_ownership::wire::DESKTOP_OWNER_HEADER,
                serde_json::to_string(owner).unwrap().parse().unwrap(),
            );
        }
        request
    }

    pub(super) async fn cleanup(mut self) {
        for ingress in self.ingresses {
            ingress.stop().await;
        }
        self.proxy.stop().await;
        self.child.take().unwrap().stop(&self.root).await;
        let identity = self.root.identity().to_owned();
        self.root.cleanup();
        // The modeled receiver retained for older component tests was never used.
        assert_eq!(self.resources.receiver.frames(), 0);
        self.resources.stores.db.drop().await.unwrap();
        self.auth.cleanup().await;
        self.audit.finish(&identity).await;
    }
}
