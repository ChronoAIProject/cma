//! Independent listeners, authenticators and Mongo clients; only durable state
//! and the fake identity provider are shared between simulated replicas.
use super::{fake_nyxid::FakeNyxid, support};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use cmaeg::{
    auth::CreatorAuthenticator,
    console::{
        clock::SystemClock,
        directory::NoChannels,
        session_backend::{schema, MongoSessionBackend, SessionBackend},
        source_changes::PendingSourceChanges,
        Console, ConsoleDeps,
    },
    ConsoleConfig,
};
use cmaeg_core::seal::MasterKeyRing;
use mongodb::{
    bson::{doc, Document},
    Client, Database,
};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tokio::sync::oneshot;

pub const ORIGIN: &str = "https://console.example";

pub struct Cluster {
    pub db: Database,
    pub nyxid: FakeNyxid,
    pub keys: Arc<MasterKeyRing>,
    uri: String,
}

impl Cluster {
    pub async fn new() -> Self {
        let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI")
            .expect("owned authenticated Mongo fixture required");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("console_sessions_{}", uuid::Uuid::new_v4().simple());
        let db = admin.database(&name);
        schema::migrate(&db).await.unwrap();
        let role = schema::role(&name, false);
        let role_name = role.get_str("createRole").unwrap().to_owned();
        db.run_command(role).await.unwrap();
        let password = uuid::Uuid::new_v4().to_string();
        db.run_command(
            doc! {"createUser":"runtime","pwd":&password,"roles":[{"role":role_name,"db":&name}]},
        )
        .await
        .unwrap();
        let mut url = reqwest::Url::parse(&uri).unwrap();
        url.set_username("runtime").unwrap();
        url.set_password(Some(&password)).unwrap();
        url.set_path(&format!("/{name}"));
        url.set_query(Some(
            "directConnection=true&serverSelectionTimeoutMS=1500&connectTimeoutMS=1500",
        ));
        let mut key = [0; 32];
        SystemRandom::new().fill(&mut key).unwrap();
        let keys = Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode(key))).unwrap());
        Self {
            db,
            nyxid: FakeNyxid::start().await,
            keys,
            uri: url.to_string(),
        }
    }

    pub async fn backend(&self) -> Arc<dyn SessionBackend> {
        let client = Client::with_uri_str(&self.uri).await.unwrap();
        let db = client.database(self.db.name());
        schema::verify_grants(&db).await.unwrap();
        schema::verify(&db).await.unwrap();
        Arc::new(MongoSessionBackend::new(db))
    }

    pub async fn replica(&self) -> Replica {
        self.replica_with(self.backend().await, self.keys.clone())
            .await
    }

    pub async fn replica_with(
        &self,
        backend: Arc<dyn SessionBackend>,
        keys: Arc<MasterKeyRing>,
    ) -> Replica {
        Replica::start(&self.nyxid.base_url, backend, keys, support::config()).await
    }

    pub async fn due(&self, sid: &str) {
        self.db
            .collection::<Document>(schema::COLLECTION)
            .update_one(
                doc! {"_id":id(sid)},
                doc! {"$set":{"access_expires_at":mongodb::bson::DateTime::from_millis(1)}},
            )
            .await
            .unwrap();
    }

    pub async fn row(&self, sid: &str) -> Option<Document> {
        self.db
            .collection::<Document>(schema::COLLECTION)
            .find_one(doc! {"_id":id(sid)})
            .await
            .unwrap()
    }

    pub async fn close(self) {
        self.nyxid.close().await;
        tokio::time::timeout(Duration::from_secs(5), self.db.drop())
            .await
            .unwrap()
            .unwrap();
    }
}

pub fn id(sid: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(sid.as_bytes()))
}

pub struct Replica {
    pub base: String,
    pub console: Console,
    pub http: reqwest::Client,
    shutdown: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl Replica {
    async fn start(
        nyxid: &str,
        backend: Arc<dyn SessionBackend>,
        keys: Arc<MasterKeyRing>,
        config: ConsoleConfig,
    ) -> Self {
        let console = Console::with_session_backend(
            ConsoleDeps {
                config,
                public_url: ORIGIN.into(),
                public_api_url: ORIGIN.into(),
                nyxid_base_url: nyxid.into(),
                clock: Arc::new(SystemClock),
                keys,
                creator_auth: Arc::new(CreatorAuthenticator::new(nyxid).unwrap()),
                directory: Arc::new(NoChannels),
                source_changes: Arc::new(PendingSourceChanges),
                multi_consumer: false,
            },
            backend,
        )
        .unwrap();
        console.discover_once().await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (shutdown, stopped) = oneshot::channel();
        let router = console.router();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            base,
            console,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            shutdown,
            task,
        }
    }

    pub async fn login(&self, callback: &Self, cluster: &Cluster, subject: &str) -> String {
        *cluster.nyxid.oauth.login_subject.lock().unwrap() = subject.into();
        let response = self
            .http
            .get(format!("{}/console/login", self.base))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 302);
        let cookie = support::set_cookie(&response, "__Host-cmaeg-console-login").unwrap();
        let response = self
            .http
            .get(response.headers()["location"].to_str().unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 303);
        let location =
            reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
        let response = callback
            .http
            .get(format!(
                "{}{}?{}",
                callback.base,
                location.path(),
                location.query().unwrap()
            ))
            .header("cookie", format!("__Host-cmaeg-console-login={cookie}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 303);
        let header = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find(|value| value.starts_with("__Host-cmaeg-console="))
            .unwrap();
        for required in ["Secure", "HttpOnly", "SameSite=Lax", "Path=/"] {
            assert!(header.contains(required));
        }
        support::set_cookie(&response, "__Host-cmaeg-console").unwrap()
    }

    pub async fn get(&self, sid: &str) -> reqwest::Response {
        self.http
            .get(format!("{}/console", self.base))
            .header("cookie", format!("__Host-cmaeg-console={sid}"))
            .send()
            .await
            .unwrap()
    }

    pub async fn page(&self, sid: &str) -> String {
        let response = self.get(sid).await;
        assert_eq!(response.status(), 200);
        response.text().await.unwrap()
    }

    pub async fn post(
        &self,
        sid: &str,
        path: &str,
        origin: &str,
        fields: &[(&str, &str)],
    ) -> reqwest::Response {
        self.http
            .post(format!("{}{path}", self.base))
            .header("origin", origin)
            .header("cookie", format!("__Host-cmaeg-console={sid}"))
            .form(fields)
            .send()
            .await
            .unwrap()
    }

    pub async fn logout(&self, sid: &str, csrf: &str) -> reqwest::Response {
        self.post(sid, "/console/logout", ORIGIN, &[("csrf", csrf)])
            .await
    }

    pub async fn close(self) {
        let _ = self.shutdown.send(());
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap();
    }
}
