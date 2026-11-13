# S4 later-progress gate evidence

Exact author-source gate record, preserved from the handoff.

```json
{
  "source_sha": "2144909cede519ed286184a7fffe1af3e645a47b",
  "branch": "tl/cma-log-segments/writer-publication",
  "strict_clippy": {
    "exit_code": 0,
    "diagnostics": 0,
    "warnings_denied": true
  },
  "units": {
    "passed": 166,
    "failed": 0,
    "cma": 117,
    "core": 49,
    "excluded": 4027
  },
  "mongo": {
    "passed": 71,
    "failed": 0,
    "writer_publication": 40,
    "capacity_recovery": 31,
    "excluded": 2596,
    "seconds": 177.547
  },
  "resource_monitor": {
    "exit_code": 0,
    "minimum_available_ram_percent": 26,
    "pauses": 0,
    "lock_inode": 160080032
  },
  "fixtures": [
    {
      "container": "cma-mongo-ci-081365b577332a7bc6224726",
      "volume": "cma-mongo-ci-081365b577332a7bc6224726-data",
      "confirmed_absent": true
    },
    {
      "container": "cma-mongo-ci-a45ea882dc47732959de9877",
      "volume": "cma-mongo-ci-a45ea882dc47732959de9877-data",
      "confirmed_absent": true
    },
    {
      "container": "cma-mongo-ci-44cb162576e664b870f6683e",
      "volume": "cma-mongo-ci-44cb162576e664b870f6683e-data",
      "confirmed_absent": true
    }
  ],
  "light_gates": {
    "fmt": 0,
    "source_files_checked": 6204,
    "configuration_keys": [
      143,
      185,
      12
    ],
    "source_history_commits_scanned": 3908
  },
  "cleanup": {
    "owned_allocated_bytes_removed": 2451226624,
    "owned_allocated_gb_removed": 2.451,
    "removed": [
      "api/target",
      "notes/p1-3-tools",
      "notes/2026-11-12-p1-3-sequence-proof-proposal.patch"
    ],
    "worktree_retained": true,
    "shared_locks_retained": true
  }
}
```
