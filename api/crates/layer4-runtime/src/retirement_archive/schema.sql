CREATE TABLE archive_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    archive_id TEXT NOT NULL CHECK(length(archive_id) = 36),
    sandbox TEXT NOT NULL CHECK(length(sandbox) BETWEEN 1 AND 256),
    home TEXT NOT NULL CHECK(length(home) BETWEEN 1 AND 256),
    state_root TEXT NOT NULL CHECK(length(state_root) BETWEEN 1 AND 65536),
    home_root TEXT NOT NULL CHECK(length(home_root) BETWEEN 1 AND 65536),
    transfer_root TEXT NOT NULL CHECK(length(transfer_root) BETWEEN 1 AND 65536)
) STRICT;

CREATE TABLE retired_groups (
    group_key TEXT PRIMARY KEY NOT NULL CHECK(length(group_key) = 64),
    source_sha256 TEXT NOT NULL CHECK(length(source_sha256) = 64),
    observation_sha256 TEXT NOT NULL CHECK(length(observation_sha256) = 64),
    records INTEGER NOT NULL CHECK(records > 0),
    bytes INTEGER NOT NULL CHECK(bytes > 0),
    complete INTEGER NOT NULL CHECK(complete IN (0,1))
) STRICT, WITHOUT ROWID;

CREATE INDEX pending_retirements ON retired_groups(group_key) WHERE complete = 0;

CREATE TABLE archived_records (
    group_key TEXT NOT NULL REFERENCES retired_groups(group_key),
    scope BLOB NOT NULL CHECK(length(scope) = 8),
    kind INTEGER NOT NULL CHECK(kind IN (1,2)),
    source_name TEXT NOT NULL CHECK(length(source_name) = 69),
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 1048576),
    PRIMARY KEY(group_key, scope, kind, source_name)
) STRICT, WITHOUT ROWID;

CREATE TABLE retired_operations (
    scope BLOB NOT NULL CHECK(length(scope) = 8),
    operation_id TEXT NOT NULL CHECK(length(operation_id) BETWEEN 1 AND 256),
    group_key TEXT NOT NULL REFERENCES retired_groups(group_key),
    PRIMARY KEY(scope, operation_id)
) STRICT, WITHOUT ROWID;
