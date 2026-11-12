CREATE TABLE collection_evidence (
    operation_id TEXT PRIMARY KEY NOT NULL CHECK(length(operation_id) = 36),
    group_key TEXT UNIQUE NOT NULL CHECK(length(group_key) = 64),
    binding_sha256 TEXT NOT NULL CHECK(length(binding_sha256) = 64),
    plan_sha256 TEXT NOT NULL CHECK(length(plan_sha256) = 64),
    artifacts_sha256 TEXT NOT NULL CHECK(length(artifacts_sha256) = 64),
    candidate_bytes INTEGER NOT NULL CHECK(candidate_bytes BETWEEN 1 AND 2097152),
    request_bytes INTEGER NOT NULL CHECK(request_bytes BETWEEN 1 AND 65536),
    observation_bytes INTEGER NOT NULL CHECK(observation_bytes BETWEEN 1 AND 4194304),
    pending INTEGER NOT NULL CHECK(pending IN (0,1))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX one_pending_collection ON collection_evidence(pending) WHERE pending=1;

CREATE TABLE collection_pending (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    operation_id TEXT UNIQUE NOT NULL REFERENCES collection_evidence(operation_id)
) STRICT;

CREATE TABLE collection_acknowledgements (
    operation_id TEXT PRIMARY KEY NOT NULL REFERENCES collection_evidence(operation_id),
    receipt_sha256 TEXT NOT NULL CHECK(length(receipt_sha256) = 64),
    receipt BLOB NOT NULL CHECK(length(receipt) BETWEEN 1 AND 16384)
) STRICT, WITHOUT ROWID;

CREATE TABLE collection_artifacts (
    operation_id TEXT NOT NULL REFERENCES collection_evidence(operation_id),
    kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 2),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 63),
    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 65536),
    PRIMARY KEY(operation_id,kind,ordinal)
) STRICT, WITHOUT ROWID;
