use mongodb::Client;
use serde_json::json;
use sha2::{Digest, Sha256};

pub(super) struct Audit {
    client: Client,
    before: Vec<String>,
    owned: Vec<String>,
}

fn identity(name: &str) -> String {
    format!("{:x}", Sha256::digest(name.as_bytes()))
}

async fn inventory(client: &Client) -> Vec<String> {
    let mut names = client
        .list_database_names()
        .await
        .unwrap()
        .iter()
        .map(|name| identity(name))
        .collect::<Vec<_>>();
    names.sort();
    names
}

impl Audit {
    pub(super) async fn start() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let client = Client::with_uri_str(uri).await.unwrap();
        let before = inventory(&client).await;
        Self {
            client,
            before,
            owned: Vec::new(),
        }
    }

    pub(super) fn bind(&mut self, resources: &str, authentication: &str) {
        self.owned = vec![identity(resources), identity(authentication)];
        assert_ne!(self.owned[0], self.owned[1]);
        assert!(self.owned.iter().all(|name| !self.before.contains(name)));
    }

    pub(super) async fn finish(self, id: &str) {
        let after = inventory(&self.client).await;
        let owned_absent = self.owned.iter().all(|name| !after.contains(name));
        println!(
            "DESKTOP_FIXTURE_CLEANUP={}",
            json!({"schema": 1, "id": id, "before": self.before, "after": after,
                "owned": self.owned, "owned_absent": owned_absent})
        );
        assert!(owned_absent);
        assert_eq!(self.before, after);
    }
}
