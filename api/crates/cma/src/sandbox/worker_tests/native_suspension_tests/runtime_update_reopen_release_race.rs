use std::sync::Arc;

use crate::store::released_runtimes::{MongoReleasedRuntimes, StoreReleasedRuntime};
use layer2_core::released_runtime::{
    CandidateState, PromotionEvidence, ReleasedLane, ReleasedLaneRecord,
};
use layer3_adapter::{PinnedReleasedRuntime, ReleasedRuntimeSource};

use super::TARGET;

pub(super) const NEXT: &str =
    "registry.example/runtime@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const DEPLOYMENT: &str = "test";

pub(super) struct ReleaseRace {
    leader: MongoReleasedRuntimes,
    pub(super) source: Arc<StoreReleasedRuntime>,
}

impl ReleaseRace {
    pub(super) async fn new(mongo: &mongodb::Client, name: &str) -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let other = mongodb::Client::with_uri_str(uri).await.unwrap();
        let leader = MongoReleasedRuntimes::open(&other.database(name), DEPLOYMENT);
        leader.initialize().await.unwrap();
        let at = layer2_core::now_rfc3339();
        let record =
            ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, TARGET.into(), &at).unwrap();
        leader.bootstrap(record.clone()).await.unwrap();
        let enabled = record.enable_following().unwrap();
        leader.replace(&record, enabled, None).await.unwrap();
        let source = StoreReleasedRuntime::open(
            &mongo.database(name),
            DEPLOYMENT,
            PinnedReleasedRuntime::new(TARGET, None).snapshot(),
            true,
        )
        .await
        .unwrap()
        .unwrap();
        Self {
            leader,
            source: Arc::new(source),
        }
    }

    pub(super) async fn promote_after_refusal(&self) {
        let captured = self.source.snapshot();
        assert_eq!(captured.agent, TARGET);
        let current = self
            .leader
            .load(ReleasedLane::Agent)
            .await
            .unwrap()
            .unwrap();
        let observation = "reopen-race";
        let at = layer2_core::now_rfc3339();
        let prepared = current
            .observe(NEXT.into(), observation.into(), &at)
            .unwrap()
            .settle(observation, CandidateState::Ready)
            .unwrap();
        self.leader
            .replace(&current, prepared.clone(), None)
            .await
            .unwrap();
        let (promoted, transition) = prepared
            .promote(
                &PromotionEvidence {
                    image: NEXT.into(),
                    observation_id: observation.into(),
                    lane_revision: prepared.revision,
                },
                &at,
            )
            .unwrap();
        let mut session = self.leader.session().await.unwrap();
        self.leader
            .record_promotion(&prepared, promoted, transition.clone(), &mut session)
            .await
            .unwrap();
        self.leader
            .finish_promotion(&mut session, &transition)
            .await
            .unwrap();
        assert_eq!(self.source.snapshot(), captured);
    }
}
