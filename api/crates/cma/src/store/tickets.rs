//! One-time terminal attachment grants in MongoDB.

use std::time::Duration;

use layer1_public::tickets::{StoredTicket, TerminalTicketStore, TicketId};
use layer2_core::store::StoreError;
use layer2_core::{AttachmentClaim, Authorization, Capability, SessionId, SubjectId};
use mongodb::bson::{doc, Binary, DateTime};
use mongodb::options::IndexOptions;
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};

use super::durability::majority_collection;

const TICKETS: &str = "terminal_tickets";
const EXPIRY_INDEX: &str = "terminal_ticket_expiry";

#[derive(Serialize, Deserialize)]
struct TicketDoc {
    #[serde(rename = "_id")]
    id: Binary,
    session_id: String,
    subject: String,
    capabilities: Vec<String>,
    takeover: bool,
    expires_at: DateTime,
}

/// MongoDB-backed terminal tickets.
pub struct MongoTerminalTicketStore {
    tickets: Collection<TicketDoc>,
}

impl MongoTerminalTicketStore {
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let database = client.database(database);
        let store = Self {
            tickets: majority_collection(&database, TICKETS),
        };
        let expiry = IndexModel::builder()
            .keys(doc! { "expires_at": 1 })
            .options(
                IndexOptions::builder()
                    .name(EXPIRY_INDEX.to_string())
                    .expire_after(Duration::ZERO)
                    .build(),
            )
            .build();
        store
            .tickets
            .create_index(expiry)
            .await
            .map_err(|error| StoreError::Unavailable(format!("terminal ticket index: {error}")))?;
        tracing::info!("terminal ticket store ready");
        Ok(store)
    }
}

#[async_trait::async_trait]
impl TerminalTicketStore for MongoTerminalTicketStore {
    async fn put(&self, ticket: StoredTicket) -> Result<(), StoreError> {
        self.tickets
            .insert_one(to_doc(ticket))
            .await
            .map_err(|error| StoreError::Unavailable(format!("store terminal ticket: {error}")))?;
        Ok(())
    }

    async fn take(&self, id: &TicketId) -> Result<Option<StoredTicket>, StoreError> {
        self.tickets
            .find_one_and_delete(doc! { "_id": id_binary(id) })
            .await
            .map_err(|error| StoreError::Unavailable(format!("redeem terminal ticket: {error}")))?
            .map(from_doc)
            .transpose()
    }
}

fn id_binary(id: &TicketId) -> Binary {
    Binary {
        subtype: mongodb::bson::spec::BinarySubtype::Generic,
        bytes: id.as_bytes().to_vec(),
    }
}

fn to_doc(ticket: StoredTicket) -> TicketDoc {
    TicketDoc {
        id: id_binary(&ticket.id),
        session_id: ticket.session.0,
        subject: ticket.subject.0,
        capabilities: ticket
            .authorization
            .capabilities()
            .map(capability_name)
            .map(str::to_string)
            .collect(),
        takeover: matches!(ticket.claim, AttachmentClaim::Takeover),
        expires_at: DateTime::from_system_time(ticket.expires_at),
    }
}

fn from_doc(doc: TicketDoc) -> Result<StoredTicket, StoreError> {
    let id: [u8; 32] = doc.id.bytes.try_into().map_err(|bytes: Vec<u8>| {
        StoreError::Malformed(format!(
            "terminal ticket id has {} bytes instead of 32",
            bytes.len()
        ))
    })?;
    let capabilities = doc
        .capabilities
        .iter()
        .map(|name| capability(name))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StoredTicket {
        id: TicketId::from_bytes(id),
        session: SessionId(doc.session_id),
        subject: SubjectId(doc.subject),
        authorization: Authorization::from_capabilities(capabilities),
        claim: if doc.takeover {
            AttachmentClaim::Takeover
        } else {
            AttachmentClaim::Exclusive
        },
        expires_at: doc.expires_at.to_system_time(),
    })
}

fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::CreateSession => "create_session",
        Capability::UseOwnSessions => "use_own_sessions",
        Capability::TerminateOwnSession => "terminate_own_session",
        Capability::ManageOwnEnvironmentProfiles => "manage_own_environment_profiles",
        Capability::ReadAllSessions => "read_all_sessions",
        Capability::ReadRuntimeInventory => "read_runtime_inventory",
        Capability::ReadEventLog => "read_event_log",
        Capability::ReadActivityLog => "read_activity_log",
        Capability::ReadUserDirectory => "read_user_directory",
        Capability::ConfigurePlatform => "configure_platform",
        Capability::ControlAnySession => "control_any_session",
        Capability::ExceedSessionLimit => "exceed_session_limit",
        Capability::ManageOwnSponsorships => "manage_own_sponsorships",
        Capability::ReconcileSponsoredUsage => "reconcile_sponsored_usage",
    }
}

fn capability(name: &str) -> Result<Capability, StoreError> {
    match name {
        "create_session" => Ok(Capability::CreateSession),
        "use_own_sessions" => Ok(Capability::UseOwnSessions),
        "terminate_own_session" => Ok(Capability::TerminateOwnSession),
        "manage_own_environment_profiles" => Ok(Capability::ManageOwnEnvironmentProfiles),
        "read_all_sessions" => Ok(Capability::ReadAllSessions),
        "read_runtime_inventory" => Ok(Capability::ReadRuntimeInventory),
        "read_event_log" => Ok(Capability::ReadEventLog),
        "read_activity_log" => Ok(Capability::ReadActivityLog),
        "read_user_directory" => Ok(Capability::ReadUserDirectory),
        "configure_platform" => Ok(Capability::ConfigurePlatform),
        "control_any_session" => Ok(Capability::ControlAnySession),
        "exceed_session_limit" => Ok(Capability::ExceedSessionLimit),
        "manage_own_sponsorships" => Ok(Capability::ManageOwnSponsorships),
        "reconcile_sponsored_usage" => Ok(Capability::ReconcileSponsoredUsage),
        other => Err(StoreError::Malformed(format!(
            "terminal ticket names unknown capability {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use futures_util::TryStreamExt as _;

    use super::*;

    #[test]
    fn every_capability_round_trips_through_storage() {
        for capability in Capability::ALL {
            assert_eq!(
                super::capability(capability_name(capability)).unwrap(),
                capability
            );
        }
    }

    #[test]
    fn an_unknown_stored_capability_fails_closed() {
        assert!(super::capability("unknown_privilege").is_err());
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
    async fn real_mongo_has_exact_ttl_and_one_cross_instance_redeemer() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri).await.unwrap();
        let database_name = format!("chrono_ticket_test_{}", uuid::Uuid::new_v4().simple());
        let first = MongoTerminalTicketStore::open(&client, &database_name)
            .await
            .unwrap();
        let second = MongoTerminalTicketStore::open(&client, &database_name)
            .await
            .unwrap();

        let indexes = client
            .database(&database_name)
            .collection::<mongodb::bson::Document>(TICKETS)
            .list_indexes()
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let expiry = indexes
            .into_iter()
            .find(|index| {
                index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.as_deref())
                    == Some(EXPIRY_INDEX)
            })
            .expect("named ticket expiry index exists");
        assert_eq!(expiry.keys, doc! { "expires_at": 1 });
        assert_eq!(expiry.options.unwrap().expire_after, Some(Duration::ZERO));

        let ticket_id = TicketId::from_bytes([7_u8; 32]);
        first
            .put(StoredTicket {
                id: ticket_id.clone(),
                session: SessionId("sesn_a".into()),
                subject: SubjectId("subject-a".into()),
                authorization: Authorization::from_capabilities([Capability::UseOwnSessions]),
                claim: AttachmentClaim::Exclusive,
                expires_at: std::time::SystemTime::now() + Duration::from_secs(30),
            })
            .await
            .unwrap();
        let (left, right) = tokio::join!(first.take(&ticket_id), second.take(&ticket_id));
        let redeemed = [left.unwrap(), right.unwrap()];
        assert_eq!(redeemed.iter().filter(|ticket| ticket.is_some()).count(), 1);
        assert_eq!(
            redeemed
                .into_iter()
                .flatten()
                .next()
                .unwrap()
                .session
                .as_str(),
            "sesn_a"
        );

        client.database(&database_name).drop().await.unwrap();
    }
}
