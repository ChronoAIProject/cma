CREATE TABLE indexed_contexts (
    group_key TEXT PRIMARY KEY NOT NULL REFERENCES retired_groups(group_key),
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 2097152)
) STRICT, WITHOUT ROWID;

CREATE TABLE indexed_records (
    group_key TEXT NOT NULL REFERENCES indexed_contexts(group_key),
    kind INTEGER NOT NULL CHECK(kind BETWEEN 3 AND 7),
    scope BLOB NOT NULL CHECK(length(scope) = 8),
    source_key TEXT NOT NULL CHECK(length(source_key) <= 65536),
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 8388608),
    PRIMARY KEY(group_key, kind, scope, source_key)
) STRICT, WITHOUT ROWID;
