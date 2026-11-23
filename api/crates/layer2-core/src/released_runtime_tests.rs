use super::*;

fn image(digit: char) -> String {
    format!(
        "registry.example/runtime@sha256:{}",
        digit.to_string().repeat(64)
    )
}

fn ready_b() -> ReleasedLaneRecord {
    let lane = ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image('a'), "t0").unwrap();
    let lane = lane.enable_following().unwrap();
    let lane = lane.observe(image('b'), "obs-b".into(), "t1").unwrap();
    lane.settle("obs-b", CandidateState::Ready).unwrap()
}

fn evidence(lane: &ReleasedLaneRecord) -> PromotionEvidence {
    PromotionEvidence {
        image: image('b'),
        observation_id: "obs-b".into(),
        lane_revision: lane.revision,
    }
}

#[test]
fn bootstrap_requires_a_pinned_image() {
    assert!(
        ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, "runtime:latest".into(), "t").is_err()
    );
    let lane = ReleasedLaneRecord::bootstrap(ReleasedLane::Machine, image('a'), "t").unwrap();
    assert_eq!((lane.revision, lane.active.revision), (1, 1));
    assert!(lane.frozen);
    assert!(lane.observe(image('b'), "obs-b".into(), "t").is_err());
    assert!(ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image('x'), "t").is_err());
}

#[test]
fn admissions_fence_on_the_active_release_not_on_candidate_churn() {
    let lane = ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image('a'), "t0").unwrap();
    let lane = lane.enable_following().unwrap();
    let observed = lane.observe(image('b'), "obs-b".into(), "t1").unwrap();
    assert!(observed.revision > lane.revision);
    // A candidate observation does not invalidate an admission captured on A.
    let admitted = observed.admit(&image('a'), lane.active.revision).unwrap();
    assert_eq!(admitted.admissions, 1);
    assert_eq!(
        observed.admit(&image('b'), 1),
        Err(ReleasedRuntimeRefusal::Stale)
    );
}

#[test]
fn a_promotion_makes_admissions_captured_before_it_stale() {
    let lane = ready_b();
    let captured = (lane.active.image.clone(), lane.active.revision);
    let (promoted, transition) = lane.promote(&evidence(&lane), "t2").unwrap();
    assert_eq!(promoted.active.image, image('b'));
    assert_eq!(transition.from_image, image('a'));
    assert_eq!((transition.from_revision, transition.to_revision), (1, 2));
    assert_eq!(
        promoted.admit(&captured.0, captured.1),
        Err(ReleasedRuntimeRefusal::Stale)
    );
    assert!(promoted.admit(&image('b'), 2).is_ok());
}

#[test]
fn promotion_rechecks_every_fence_at_commit() {
    let lane = ready_b();
    let mut stale = evidence(&lane);
    stale.lane_revision -= 1;
    assert_eq!(
        lane.promote(&stale, "t").unwrap_err(),
        ReleasedRuntimeRefusal::Stale
    );
    let mut other = evidence(&lane);
    other.observation_id = "obs-old".into();
    assert_eq!(
        lane.promote(&other, "t").unwrap_err(),
        ReleasedRuntimeRefusal::Stale
    );
    // A newer observation supersedes the ready candidate before commit.
    let superseded = lane.observe(image('c'), "obs-c".into(), "t").unwrap();
    assert_eq!(
        superseded.promote(&evidence(&lane), "t").unwrap_err(),
        ReleasedRuntimeRefusal::Stale
    );
    let preparing = ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image('a'), "t0")
        .unwrap()
        .enable_following()
        .unwrap()
        .observe(image('b'), "obs-b".into(), "t1")
        .unwrap();
    assert_eq!(
        preparing.promote(&evidence(&preparing), "t").unwrap_err(),
        ReleasedRuntimeRefusal::NotReady
    );
}

#[test]
fn freeze_fences_promotion_across_an_explicit_reenable() {
    let lane = ready_b();
    let frozen = lane.freeze().unwrap();
    assert!(frozen.frozen);
    let enabled = frozen.enable_following().unwrap();
    assert_eq!(
        enabled.promote(&evidence(&lane), "t").unwrap_err(),
        ReleasedRuntimeRefusal::Stale
    );
    assert_eq!(
        frozen.promote(&evidence(&frozen), "t").unwrap_err(),
        ReleasedRuntimeRefusal::Frozen
    );
    // An in-flight promotion decided before the freeze is stale as well.
    assert_eq!(
        frozen.promote(&evidence(&lane), "t").unwrap_err(),
        ReleasedRuntimeRefusal::Frozen
    );
    assert_eq!(
        frozen.observe(image('c'), "obs-c".into(), "t").unwrap_err(),
        ReleasedRuntimeRefusal::Frozen
    );
    // A frozen lane keeps serving its active release.
    assert!(frozen.admit(&image('a'), 1).is_ok());
}

#[test]
fn observing_the_active_image_clears_the_candidate_and_repeats_are_idempotent() {
    let lane = ready_b();
    assert_eq!(
        lane.observe(image('b'), "obs-b2".into(), "t").unwrap(),
        lane
    );
    let cleared = lane.observe(image('a'), "obs-a".into(), "t").unwrap();
    assert!(cleared.candidate.is_none());
    assert_eq!(
        cleared.settle("obs-b", CandidateState::Ready),
        Err(ReleasedRuntimeRefusal::Stale)
    );
}

#[test]
fn the_record_round_trips_and_refuses_unknown_fields() {
    let lane = ready_b();
    let value = serde_json::to_value(&lane).unwrap();
    assert_eq!(
        serde_json::from_value::<ReleasedLaneRecord>(value.clone()).unwrap(),
        lane
    );
    let mut extra = value;
    extra["surprise"] = true.into();
    assert!(serde_json::from_value::<ReleasedLaneRecord>(extra).is_err());
}
