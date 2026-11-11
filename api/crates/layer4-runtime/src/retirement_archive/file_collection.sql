CREATE TABLE file_collection_source (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision > 0),
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
    bytes INTEGER NOT NULL CHECK(bytes BETWEEN 1 AND 25165824)
) STRICT;

CREATE TABLE file_collection_snapshot (
    ordinal INTEGER PRIMARY KEY CHECK(ordinal BETWEEN 0 AND 383),
    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 65536)
) STRICT;
