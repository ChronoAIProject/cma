CREATE TABLE metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    sandbox TEXT NOT NULL CHECK(length(sandbox) BETWEEN 1 AND 256),
    home TEXT NOT NULL CHECK(length(home) <= 256),
    generation BLOB NOT NULL CHECK(length(generation) = 8 AND generation > x'0000000000000000')
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
