use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ring::rand::{SecureRandom, SystemRandom};

pub(super) struct Database {
    pub database: mongodb::Database,
    pub stores: crate::store::MongoStores,
    pub connection: String,
    pub encryption: String,
}

impl Database {
    pub async fn open() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let database = client.database(&format!("cma_ticket19_{}", uuid::Uuid::new_v4().simple()));
        let mut bytes = [0_u8; 32];
        SystemRandom::new().fill(&mut bytes).unwrap();
        let encoded = STANDARD.encode(bytes);
        let encryption = crate::store::AuthEncryptionKey::from_base64(&encoded).unwrap();
        database
            .run_command(crate::store::isolation::role(database.name()))
            .await
            .unwrap();
        let password = uuid::Uuid::new_v4().to_string();
        database
            .run_command(mongodb::bson::doc! {
                "createUser":"ticket19-runtime", "pwd":&password,
                "roles":[{"role":"cma_am_runtime", "db":database.name()}]
            })
            .await
            .unwrap();
        let mut connection = reqwest::Url::parse(&uri).unwrap();
        connection.set_username("ticket19-runtime").unwrap();
        connection.set_password(Some(&password)).unwrap();
        connection
            .query_pairs_mut()
            .append_pair("authSource", database.name());
        let stores = crate::store::MongoStores::connect(
            connection.as_str(),
            database.name(),
            DEPLOYMENT_ID,
            &encryption,
            Duration::from_secs(3600),
            (&desktop::image(), None),
        )
        .await
        .unwrap();
        Self {
            database,
            stores,
            connection: connection.to_string(),
            encryption: encoded,
        }
    }

    pub async fn reopen(context: &Value) -> Self {
        let connection = context["connection"].as_str().unwrap().to_owned();
        let encoded = context["encryption"].as_str().unwrap().to_owned();
        let client = mongodb::Client::with_uri_str(&connection).await.unwrap();
        let database = client.database(context["database"].as_str().unwrap());
        let encryption = crate::store::AuthEncryptionKey::from_base64(&encoded).unwrap();
        let stores = crate::store::MongoStores::connect(
            &connection,
            database.name(),
            DEPLOYMENT_ID,
            &encryption,
            Duration::from_secs(3600),
            (&desktop::image(), None),
        )
        .await
        .unwrap();
        Self {
            database,
            stores,
            connection,
            encryption: encoded,
        }
    }
}
