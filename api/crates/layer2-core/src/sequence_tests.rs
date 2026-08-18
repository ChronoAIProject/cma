//! What must be true of the number a session's events are ordered by.

use super::Sequences;
use crate::ids::SessionId;

fn session() -> SessionId {
    SessionId("sesn_1".into())
}

#[test]
fn two_producers_never_agree_on_a_number() {
    // The production failure. The conversation fold numbered off the entry and
    // provisioning numbered off the record; both had seen 8, both chose 9, and
    // the batch the index refused was the turn's.
    let sequences = Sequences::default();
    let id = session();

    let from_the_entry = sequences.take(&id, 8, 1);
    let from_the_record = sequences.take(&id, 8, 1);

    assert_eq!(from_the_entry, 9);
    assert_eq!(
        from_the_record, 10,
        "a second producer holding the same stale position must not be handed the same number"
    );
}

#[test]
fn a_block_is_contiguous_and_the_next_one_clears_it() {
    let sequences = Sequences::default();
    let id = session();

    let first = sequences.take(&id, 0, 3);
    assert_eq!(first, 1, "the first sequence of the block");

    assert_eq!(
        sequences.take(&id, 0, 1),
        4,
        "the whole block is spent, not just its first"
    );
}

#[test]
fn a_stale_floor_cannot_pull_the_sequence_backwards() {
    // Every store-path writer holds a copy read at some earlier moment. If the
    // lower of the two won, a number already handed out would be handed out
    // again, and the log would carry two events answering to one cursor.
    let sequences = Sequences::default();
    let id = session();

    assert_eq!(sequences.take(&id, 40, 1), 41);
    assert_eq!(sequences.take(&id, 2, 1), 42);
    assert_eq!(sequences.issued(&id, 2), 42);
}

#[test]
fn a_higher_floor_is_taken_from_whoever_knows_it() {
    // How a process that has just started catches up: it has issued nothing,
    // and the record it reads is what tells it where the log already is.
    let sequences = Sequences::default();
    let id = session();

    assert_eq!(sequences.take(&id, 100, 1), 101);
}

#[test]
fn sessions_do_not_share_a_position() {
    let sequences = Sequences::default();
    let one = SessionId("sesn_1".into());
    let two = SessionId("sesn_2".into());

    assert_eq!(sequences.take(&one, 0, 1), 1);
    assert_eq!(sequences.take(&two, 0, 1), 1);
    assert_eq!(sequences.take(&one, 0, 1), 2);
}

#[test]
fn a_deleted_session_is_forgotten_and_a_stopped_one_is_not() {
    // A stopped session can be revived and its log continues; forgetting its
    // position would restart numbering over events that are still there.
    // A deleted session's events go with it, so keeping the position would be
    // holding a number for something nothing can read.
    let sequences = Sequences::default();
    let id = session();

    assert_eq!(sequences.take(&id, 0, 5), 1);
    assert_eq!(sequences.issued(&id, 0), 5);

    sequences.forget(&id);

    assert_eq!(
        sequences.issued(&id, 0),
        0,
        "nothing is remembered about a session that no longer exists"
    );
}

#[test]
fn a_poisoned_lock_still_hands_out_a_number() {
    // A panic somewhere else must not take the sequence with it: every event
    // in this process would stop being writable, which is a worse outcome than
    // continuing from a position that is still perfectly consistent.
    let sequences = std::sync::Arc::new(Sequences::default());
    let id = session();
    sequences.take(&id, 7, 1);

    let poisoner = std::sync::Arc::clone(&sequences);
    let panicked = std::thread::spawn(move || {
        let _held = poisoner.issued.lock().expect("held");
        panic!("something else failed");
    })
    .join();
    assert!(panicked.is_err(), "the helper must actually have panicked");

    assert_eq!(sequences.take(&id, 0, 1), 9);
}
