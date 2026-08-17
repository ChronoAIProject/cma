//! Carrying a conversation from one pod to the next.
//!
//! The round trip is what §5 stands on, so both halves are checked here: what
//! one pod captured is what the next one lays down, and where.

use super::*;
use crate::carry;

/// The conversation inside a captured envelope, and where it was found.
fn carried(sealed: &[u8]) -> (Option<PathBuf>, Vec<u8>) {
    let (path, payload) = carry::open(sealed);
    (path, payload.to_vec())
}

/// A home directory of this test's own, matching how the sibling module
/// does it rather than taking a dependency to say the same thing.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("chrono-conversation-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a temporary home");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A stand-in for whatever an adapter tells this pod its kernel uses.
const SESSIONS_DIR: &str = ".a-kernel/sessions";

fn write_rollout(home: &Path, day: &str, name: &str, body: &str) -> PathBuf {
    let dir = home.join(SESSIONS_DIR).join(day);
    std::fs::create_dir_all(&dir).expect("directories");
    let path = dir.join(name);
    std::fs::write(&path, body).expect("rollout");
    path
}

#[test]
fn a_session_that_never_ran_has_nothing_to_carry() {
    // Not a failure: a pod may be replaced before anyone has used it, and
    // treating that as an error would refuse to move a healthy session.
    let dir = Home::new("never-ran");
    assert!(capture(dir.path(), SESSIONS_DIR)
        .expect("readable")
        .is_none());
}

#[test]
fn the_conversation_is_carried_verbatim() {
    // Byte-exact on purpose. Anything that rewrites the agent's own file
    // is something that breaks when the agent changes its format.
    let dir = Home::new("verbatim");
    let body = "{\"turn\":1}\n{\"turn\":2}\n";
    write_rollout(dir.path(), "2026/08/14", "rollout-a.jsonl", body);

    let (path, payload) = carried(
        &capture(dir.path(), SESSIONS_DIR)
            .expect("readable")
            .expect("present"),
    );
    assert_eq!(payload, body.as_bytes());
    assert_eq!(
        path.as_deref(),
        Some(Path::new("2026/08/14/rollout-a.jsonl")),
        "and where it was found travels with it"
    );
}

#[test]
fn the_newest_conversation_is_the_one_carried() {
    // A pod hosts one session, so older rollouts are finished
    // conversations. Carrying one of those would resume the wrong thing.
    let dir = Home::new("newest");
    write_rollout(dir.path(), "2026/08/13", "rollout-old.jsonl", "old");
    let newer = write_rollout(dir.path(), "2026/08/14", "rollout-new.jsonl", "new");
    // Modification times decide, and a freshly written file may share a
    // timestamp with its sibling, so this is made explicit.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(&newer)
        .expect("openable")
        .set_modified(later)
        .expect("timestamps are settable");

    let (_, payload) = carried(
        &capture(dir.path(), SESSIONS_DIR)
            .expect("readable")
            .expect("present"),
    );
    assert_eq!(payload, b"new");
}

#[test]
fn what_is_carried_does_not_depend_on_what_a_file_is_called() {
    // The supervisor hosts any kernel, and kernels do not agree on how a
    // conversation is stored — one writes newline-delimited JSON, another
    // compressed frames. Filtering on a filename would be this module
    // knowing a format, which is the thing it exists not to know.
    //
    // The safer failure runs this way round. Carrying something the next
    // agent cannot read is caught downstream and refuses the rotation,
    // leaving the session where it is; carrying nothing because the name
    // was unfamiliar reads as "no conversation" and loses the history for
    // real.
    let dir = Home::new("any-filename");
    write_rollout(dir.path(), "2026/08/14", "store.zst", "carried anyway");
    let (_, payload) = carried(
        &capture(dir.path(), SESSIONS_DIR)
            .expect("readable")
            .expect("present"),
    );
    assert_eq!(payload, b"carried anyway");
}

#[test]
fn a_conversation_that_names_itself_in_another_shape_still_restores() {
    // The first-line `payload.id` belongs to one kernel's rollout format.
    // A kernel that records something else — or something that is not text
    // at all — has no identifier to give, and that is an ordinary answer.
    // Failing here instead would make this module's one piece of format
    // knowledge into a requirement every kernel has to satisfy.
    let dir = Home::new("foreign-shape");
    let restored = restore(
        dir.path(),
        SESSIONS_DIR,
        &[0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x01],
        "2026-08-17T00-00-00",
    )
    .expect("a conversation in an unfamiliar shape is still laid down");
    assert!(restored.path.exists());
    assert!(
        restored.session_id.is_none(),
        "there is no identifier to invent"
    );
}

/// A rollout's opening record, in the shape the agent writes it.
const META: &str = r#"{"timestamp":"2026-08-14T00:00:00Z","type":"session_meta","payload":{"id":"019ed5ef-b5eb-7f60-96d1-8b8202fa7eff","cwd":"/somewhere/else"}}"#;

#[test]
fn a_restored_conversation_names_itself() {
    // The identifier is the whole mechanism: the agent continues a
    // conversation by name, and asked for "the most recent" it would
    // select among conversations recorded in this working directory —
    // which a conversation carried from another pod never is.
    let dir = Home::new("names-itself");
    let restored = restore(
        dir.path(),
        SESSIONS_DIR,
        format!("{META}\n").as_bytes(),
        "stamp",
    )
    .expect("restorable");
    assert_eq!(
        restored.session_id.as_deref(),
        Some("019ed5ef-b5eb-7f60-96d1-8b8202fa7eff")
    );
}

#[test]
fn a_conversation_that_does_not_name_itself_is_not_guessed_at() {
    // Better to start fresh loudly than to resume something arbitrary.
    let dir = Home::new("unnamed");
    let restored =
        restore(dir.path(), SESSIONS_DIR, b"not json at all\n", "stamp").expect("restorable");
    assert!(restored.session_id.is_none());
}

#[test]
fn a_restored_conversation_is_what_the_agent_finds() {
    // The round trip is the whole point: what one pod captured is what the
    // next one resumes from.
    let source = Home::new("round-trip-source");
    let body = "{\"turn\":1}\n";
    write_rollout(source.path(), "2026/08/14", "rollout-a.jsonl", body);
    let captured = capture(source.path(), SESSIONS_DIR)
        .expect("readable")
        .expect("present");

    let target = Home::new("round-trip-target");
    restore(
        target.path(),
        SESSIONS_DIR,
        &captured,
        "2026-08-14T00-00-00",
    )
    .expect("restorable");

    let (path, payload) = carried(
        &capture(target.path(), SESSIONS_DIR)
            .expect("readable")
            .expect("present"),
    );
    assert_eq!(payload, body.as_bytes());
    assert_eq!(
        path.as_deref(),
        Some(Path::new("2026/08/14/rollout-a.jsonl")),
        "a second handover carries the same placement on again"
    );
}

#[test]
fn restoring_creates_the_directory_the_agent_reads() {
    // A fresh pod has no sessions tree at all, so restore cannot assume one.
    let dir = Home::new("round-trip");
    let restored =
        restore(dir.path(), SESSIONS_DIR, b"{}", "2026-08-14T00-00-00").expect("restorable");
    assert!(restored.path.exists());
    assert!(restored.path.starts_with(dir.path().join(SESSIONS_DIR)));
}

#[test]
fn an_implausibly_large_conversation_is_refused() {
    // The bytes cross an HTTP hop and land on a pod's disk. Reading
    // something unbounded is a way to exhaust either.
    let dir = Home::new("creates-dir");
    let path = write_rollout(dir.path(), "2026/08/14", "rollout-a.jsonl", "x");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("openable");
    file.set_len(MAX_ROLLOUT_BYTES + 1).expect("resizable");

    assert!(capture(dir.path(), SESSIONS_DIR).is_err());
}

#[test]
fn a_store_laid_out_as_a_keyed_tree_is_rebuilt_where_it_was() {
    // The defect this closes. One kernel files a conversation under a
    // directory named for the working directory it started in and another
    // under one named for the session; both open that exact path rather than
    // scanning. Put back anywhere else, neither finds it — and the agent
    // starts a fresh conversation and reports success.
    let source = Home::new("keyed-source");
    let body = "{\"event\":1}\n";
    write_rollout(
        source.path(),
        "--home-chrono--/sesn-1",
        "session.jsonl",
        body,
    );
    let captured = capture(source.path(), SESSIONS_DIR)
        .expect("readable")
        .expect("present");

    let target = Home::new("keyed-target");
    let restored = restore(target.path(), SESSIONS_DIR, &captured, "stamp").expect("restorable");

    assert_eq!(
        restored.path,
        target
            .path()
            .join(SESSIONS_DIR)
            .join("--home-chrono--/sesn-1/session.jsonl"),
        "the path a kernel opens must be the path it is given back"
    );
    assert_eq!(
        std::fs::read(&restored.path).expect("readable"),
        body.as_bytes()
    );
}

#[test]
fn a_conversation_from_a_pod_that_predates_the_envelope_lands_where_it_always_did() {
    // A rollout is in flight between pods of different vintages exactly when
    // §5 matters most. Bare bytes are the older dialect, not a failure, and
    // what they get is today's placement — which the first kernel accepts.
    let dir = Home::new("older-dialect");
    let restored = restore(
        dir.path(),
        SESSIONS_DIR,
        b"{\"turn\":1}\n",
        "2026-08-14T00-00-00",
    )
    .expect("restorable");
    assert_eq!(
        restored.path,
        dir.path()
            .join(SESSIONS_DIR)
            .join("restored/rollout-2026-08-14T00-00-00.jsonl")
    );
    assert_eq!(
        std::fs::read(&restored.path).expect("readable"),
        b"{\"turn\":1}\n",
        "and its conversation must not have a header eaten off the front"
    );
}

#[test]
fn a_carried_conversation_still_names_itself_through_its_envelope() {
    // The identifier is read from the agent's own first line, which now has a
    // line of ours in front of it. Reading the wrong one would tell a kernel
    // to continue a conversation that does not exist.
    let source = Home::new("names-through-envelope");
    write_rollout(
        source.path(),
        "2026/08/14",
        "rollout-a.jsonl",
        &format!("{META}\n"),
    );
    let captured = capture(source.path(), SESSIONS_DIR)
        .expect("readable")
        .expect("present");

    let target = Home::new("names-through-envelope-target");
    let restored = restore(target.path(), SESSIONS_DIR, &captured, "stamp").expect("restorable");
    assert_eq!(
        restored.session_id.as_deref(),
        Some("019ed5ef-b5eb-7f60-96d1-8b8202fa7eff")
    );
}
