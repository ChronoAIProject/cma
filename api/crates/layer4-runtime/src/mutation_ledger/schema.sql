CREATE TABLE metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    sandbox TEXT NOT NULL CHECK(length(sandbox) BETWEEN 1 AND 256),
    home TEXT NOT NULL CHECK(length(home) <= 256),
    generation BLOB NOT NULL CHECK(length(generation) = 8 AND generation > x'0000000000000000'),
    state_root TEXT NOT NULL CHECK(length(state_root) BETWEEN 1 AND 65536),
    home_root TEXT NOT NULL CHECK(length(home_root) BETWEEN 1 AND 65536),
    transfer_root TEXT NOT NULL CHECK(length(transfer_root) BETWEEN 1 AND 65536),
    store_id BLOB NOT NULL CHECK(length(store_id) = 16),
    admission TEXT NOT NULL CHECK(admission IN ('fresh','import_held'))
) STRICT;

CREATE TABLE authority (
    scope BLOB NOT NULL CHECK(length(scope) = 8),
    resource TEXT NOT NULL CHECK(length(resource) BETWEEN 1 AND 65536),
    high_water BLOB NOT NULL CHECK(length(high_water) = 8 AND high_water > x'0000000000000000'),
    present BLOB CHECK(present IS NULL OR length(present) BETWEEN 1 AND 65536),
    first_absent BLOB CHECK(first_absent IS NULL OR length(first_absent) BETWEEN 1 AND 65536),
    last_absent BLOB CHECK(last_absent IS NULL OR length(last_absent) BETWEEN 1 AND 65536),
    CHECK(present IS NOT NULL OR first_absent IS NOT NULL),
    CHECK((first_absent IS NULL) = (last_absent IS NULL)),
    PRIMARY KEY(scope, resource)
) STRICT, WITHOUT ROWID;


CREATE TABLE accounting (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision >= 0),
    reserved INTEGER NOT NULL CHECK(reserved >= 0),
    operations INTEGER NOT NULL CHECK(operations >= 0),
    authority INTEGER NOT NULL CHECK(authority >= 0),
    terminal INTEGER NOT NULL CHECK(terminal >= 0)
) STRICT;

CREATE TABLE scope_budget (
    scope BLOB PRIMARY KEY NOT NULL CHECK(length(scope) = 8),
    pending INTEGER NOT NULL CHECK(pending >= 0),
    processes INTEGER NOT NULL CHECK(processes >= 0)
) STRICT, WITHOUT ROWID;

CREATE TABLE cleanup_budget (
    scope BLOB NOT NULL CHECK(length(scope) = 8),
    resource TEXT NOT NULL CHECK(length(resource) BETWEEN 1 AND 65536),
    available INTEGER NOT NULL CHECK(available IN (0,1)),
    PRIMARY KEY(scope, resource)
) STRICT, WITHOUT ROWID;

CREATE TABLE operations (
    scope BLOB NOT NULL CHECK(length(scope) = 8),
    operation_id TEXT NOT NULL CHECK(length(operation_id) BETWEEN 1 AND 256),
    identity BLOB NOT NULL CHECK(length(identity) BETWEEN 1 AND 65536),
    status INTEGER NOT NULL CHECK(status BETWEEN 0 AND 4),
    normal INTEGER NOT NULL CHECK(normal IN (0,1)),
    record BLOB CHECK(record IS NULL OR length(record) BETWEEN 1 AND 1048576),
    PRIMARY KEY(scope, operation_id),
    CHECK((status = 0 AND record IS NULL) OR (status > 0 AND record IS NOT NULL)),
    CHECK(record IS NULL OR json_valid(CAST(record AS TEXT))),
    CHECK(record IS NULL OR (
        (status = 1 AND json_extract(CAST(record AS TEXT),'$.status') IS 'started'
          AND json_type(CAST(record AS TEXT),'$.receipt') IS NULL AND json_type(CAST(record AS TEXT),'$.failure') IS NULL)
        OR (status = 2 AND json_extract(CAST(record AS TEXT),'$.status') IS 'completed'
          AND json_type(CAST(record AS TEXT),'$.receipt') IS 'object' AND json_type(CAST(record AS TEXT),'$.failure') IS NULL)
        OR (status IN (3,4) AND json_extract(CAST(record AS TEXT),'$.status') IS CASE status WHEN 3 THEN 'failed' ELSE 'indeterminate' END
          AND json_type(CAST(record AS TEXT),'$.receipt') IS NULL AND json_type(CAST(record AS TEXT),'$.failure') IS 'object'
          AND json_extract(CAST(record AS TEXT),'$.failure.retryable') IS 0)
    ))
) STRICT, WITHOUT ROWID;

CREATE INDEX outstanding ON operations(scope, status, operation_id) WHERE status IN (0,1);


CREATE TABLE logical_processes (
    scope BLOB NOT NULL CHECK(length(scope) = 8),
    agent TEXT NOT NULL CHECK(length(agent) BETWEEN 1 AND 256),
    cwd TEXT NOT NULL CHECK(length(cwd) BETWEEN 1 AND 65536),
    home TEXT NOT NULL CHECK(length(home) BETWEEN 1 AND 65536),
    PRIMARY KEY(scope, agent)
) STRICT, WITHOUT ROWID;

CREATE INDEX process_working_path ON logical_processes(scope, cwd, agent);
