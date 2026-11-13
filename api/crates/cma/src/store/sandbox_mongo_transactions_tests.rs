use super::*;
use std::collections::VecDeque;

struct Sequence {
    outcomes: VecDeque<Result<(), CommitFailure<&'static str>>>,
    attempts: usize,
}

#[async_trait::async_trait]
impl CommitOperation for Sequence {
    type Error = &'static str;

    async fn commit(&mut self) -> Result<(), CommitFailure<Self::Error>> {
        self.attempts += 1;
        self.outcomes.pop_front().expect("unexpected commit retry")
    }
}

async fn sequence(
    outcomes: Vec<Result<(), CommitFailure<&'static str>>>,
) -> (CommitResolution<&'static str>, usize) {
    let mut sequence = Sequence {
        outcomes: outcomes.into(),
        attempts: 0,
    };
    let result = commit_with(&mut sequence).await;
    (result, sequence.attempts)
}

#[tokio::test]
async fn acknowledged_commit_finishes_without_repeating_mutation() {
    let (result, attempts) = sequence(vec![Ok(())]).await;
    assert!(matches!(result, CommitResolution::Committed));
    assert_eq!(attempts, 1);
    let (result, attempts) = sequence(vec![
        Err(CommitFailure::Unknown("lost acknowledgement")),
        Ok(()),
    ])
    .await;
    assert!(matches!(result, CommitResolution::Committed));
    assert_eq!(attempts, 2);
}

#[tokio::test]
async fn commit_uncertainty_cannot_be_erased_by_a_later_failure() {
    for later in [
        CommitFailure::Transient("transient after unknown"),
        CommitFailure::Failed("failure after unknown"),
    ] {
        let (result, attempts) = sequence(vec![
            Err(CommitFailure::Unknown("lost acknowledgement")),
            Err(later),
        ])
        .await;
        assert!(matches!(result, CommitResolution::Unknown(_)));
        assert_eq!(attempts, 2);
    }
}

#[tokio::test]
async fn known_commit_failures_do_not_retry_the_commit() {
    let (result, attempts) = sequence(vec![Err(CommitFailure::Transient("aborted"))]).await;
    assert!(matches!(result, CommitResolution::Retry("aborted")));
    assert_eq!(attempts, 1);
    let (result, attempts) = sequence(vec![Err(CommitFailure::Failed("refused"))]).await;
    assert!(matches!(result, CommitResolution::Failed("refused")));
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn repeated_unknown_commits_exhaust_the_bound_without_replay() {
    let (result, attempts) = sequence(
        (0..COMMIT_ATTEMPTS)
            .map(|_| Err(CommitFailure::Unknown("unresolved")))
            .collect(),
    )
    .await;
    assert!(matches!(result, CommitResolution::Unknown("unresolved")));
    assert_eq!(attempts, COMMIT_ATTEMPTS);
}
