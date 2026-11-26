//! A reserved owner turn for exercising exact Stop settlement.

use super::{budget, MongoSandboxStore};
use layer2_core::{
    budget::{BudgetPolicy, OwnerBudget, TurnReservation},
    sandbox::Agent,
};
use mongodb::bson::{self, doc, Document};

pub(crate) struct OwnerStopBudgetFixture {
    turn_key: String,
    owner_key: String,
    held_turn: Document,
    held_owner: Document,
    reserved_units: u64,
}

impl OwnerStopBudgetFixture {
    pub(crate) async fn reserve(store: &MongoSandboxStore, agent: &Agent, turn: u64) -> Self {
        let turn_key = budget::turn_key(agent, turn);
        let owner_key = format!("owner:{}", agent.owner);
        let policy = BudgetPolicy {
            owner_day_limit: 100,
            turn_limit: 10,
            max_calls_per_turn: 5,
            day_timezone: "UTC".into(),
            input_token_units: 1,
            output_token_units: 1,
            max_unobserved_units: 2,
            usage_timeout_seconds: 60,
        };
        let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
        let mut owner = OwnerBudget::default();
        owner.reserve(&policy, now).unwrap();
        let reservation = TurnReservation::new(policy, now);
        let held_turn = doc! {
            "_id": &turn_key, "document_schema_version": 1_i64,
            "record": {
                "owner": agent.owner.as_str(), "agent_id": agent.id.as_str(),
                "turn_sequence": i64::try_from(turn).unwrap(),
                "runtime_generation": i64::try_from(agent.conversation.runtime_generation.unwrap()).unwrap(),
                "reservation": bson::serialize_to_bson(&reservation).unwrap(),
            },
        };
        let held_owner = doc! {
            "_id": &owner_key, "document_schema_version": 1_i64,
            "record": bson::serialize_to_bson(&owner).unwrap(),
        };
        store
            .model_budget
            .insert_many([held_turn.clone(), held_owner.clone()])
            .await
            .unwrap();
        Self {
            turn_key,
            owner_key,
            held_turn,
            held_owner,
            reserved_units: reservation.reserved_units,
        }
    }

    pub(crate) async fn assert_held(&self, store: &MongoSandboxStore) {
        let (turn, owner) = self.read(store).await;
        assert_eq!(turn, self.held_turn);
        assert_eq!(owner, self.held_owner);
    }

    pub(crate) async fn assert_settled_once(&self, store: &MongoSandboxStore) {
        let (turn, owner) = self.read(store).await;
        assert!(turn
            .get_document("record")
            .unwrap()
            .get_document("reservation")
            .unwrap()
            .get_bool("settled")
            .unwrap());
        let owner: OwnerBudget =
            bson::deserialize_from_document(owner.get_document("record").unwrap().clone()).unwrap();
        assert_eq!((owner.reserved, owner.spent), (0, self.reserved_units));
    }

    async fn read(&self, store: &MongoSandboxStore) -> (Document, Document) {
        let turn = store
            .model_budget
            .find_one(doc! {"_id": &self.turn_key})
            .await
            .unwrap()
            .unwrap();
        let owner = store
            .model_budget
            .find_one(doc! {"_id": &self.owner_key})
            .await
            .unwrap()
            .unwrap();
        (turn, owner)
    }
}
