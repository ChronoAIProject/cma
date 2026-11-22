use super::*;
use mongodb::options::{ReadConcern, ReadPreference, SelectionCriteria};

const READ_BUDGET: Duration = Duration::from_secs(2);
const COMMAND_BUDGET: Duration = Duration::from_millis(750);

pub(super) async fn erasing(
    store: &MongoAccountDeletionStore,
    subject: &SubjectId,
) -> Result<bool, AccountDeletionError> {
    tokio::time::timeout(READ_BUDGET, read(store, subject))
        .await
        .map_err(|_| AccountDeletionError::Unavailable)?
}

async fn read(
    store: &MongoAccountDeletionStore,
    subject: &SubjectId,
) -> Result<bool, AccountDeletionError> {
    // Each invocation starts without operationTime: Mongo does not permit
    // afterClusterTime on the first, linearizable read.
    let mut session = store
        .database
        .client()
        .start_session()
        .causal_consistency(true)
        .await
        .map_err(driver)?;
    if session.operation_time().is_some() {
        return Err(AccountDeletionError::Unavailable);
    }
    // Acceptance updates this unique subject fence in the receipt transaction.
    // Its primary read is the barrier; the history predicate is not unique and
    // cannot itself carry Mongo's single-document linearizability guarantee.
    store
        .database
        .collection::<Document>("sandbox_admissions")
        .find_one(doc! {"_id":subject.as_str()})
        .read_concern(ReadConcern::linearizable())
        .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
        .max_time(COMMAND_BUDGET)
        .projection(doc! {"_id":1})
        .session(&mut session)
        .await
        .map_err(driver)?;
    if session.operation_time().is_none() {
        return Err(AccountDeletionError::Unavailable);
    }
    // The causal session carries the barrier's operationTime as afterClusterTime,
    // so a majority status read cannot precede an already acknowledged receipt.
    store
        .records
        .find_one(subject_erasing_filter(subject))
        .read_concern(ReadConcern::majority())
        .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
        .max_time(COMMAND_BUDGET)
        .projection(doc! {"_id":0,"subject_hash":1,"active_subject_hash":1,"status":1})
        .session(&mut session)
        .await
        .map(|row| row.is_some())
        .map_err(driver)
}
