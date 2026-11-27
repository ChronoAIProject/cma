"""Read bounded evidence from the Runtime's own mutation ledger."""
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import sys

MAX_RECORD = 1024 * 1024


def observe(root, sandbox_id, agent_id, generation):
    operations = (
        ('worktree:workspace:wt-1:add:v1', 'add_worktree', 0),
        (f'agent:{agent_id}:home:create:v1', 'create_agent_home', 0),
        (f'agent:{agent_id}:open:v1', 'open_agent', generation),
    )
    indexed = (root / 'mutation-ledger-v2.current.json').is_file()
    if indexed:
        database = root / 'mutation-ledger-v2' / 'ledger.sqlite3'
        with closing(sqlite3.connect(database.as_uri() + '?mode=ro', uri=True, timeout=2)) as db:
            db.execute('BEGIN')
            records = [indexed_record(db, operation_id, scope) for operation_id, _, scope in operations]
    else:
        records = [legacy_record(root, operation_id, scope) for operation_id, _, scope in operations]
    for (operation_id, kind, scope), raw in zip(operations, records, strict=True):
        record = json.loads(raw)
        subject = record.get('subject', {})
        if (record.get('sandbox_id') != sandbox_id or record.get('operation_id') != operation_id
                or record.get('status') != 'completed' or not isinstance(record.get('receipt'), dict)
                or record.get('runtime_generation') != (scope or None)
                or subject.get('kind') != kind
                or (kind != 'add_worktree' and subject.get('agent_id') != agent_id)
                or not re.fullmatch(r'[a-f0-9]{64}', record.get('request_sha256', ''))):
            raise ValueError('prepared_runtime_mutation_unbound')
    return {'format': 'indexed' if indexed else 'legacy',
            'receipts': [hashlib.sha256(raw).hexdigest() for raw in records]}


def indexed_record(db, operation_id, scope):
    rows = db.execute('SELECT scope, status, CASE WHEN length(record)<=? THEN record END '
                      'FROM operations WHERE operation_id=? LIMIT 2', (MAX_RECORD, operation_id)).fetchall()
    if (len(rows) != 1 or rows[0][0] != scope.to_bytes(8, 'big') or rows[0][1] != 2
            or not isinstance(rows[0][2], bytes) or not rows[0][2]):
        raise ValueError('prepared_runtime_indexed_mutation_unproven')
    return rows[0][2]


def legacy_record(root, operation_id, scope):
    if scope:
        root = root / f'process-generation-{scope}'
    name = hashlib.sha256(operation_id.encode()).hexdigest() + '.json'
    with (root / 'runtime-mutations' / name).open('rb') as source:
        raw = source.read(MAX_RECORD + 1)
    if not raw or len(raw) > MAX_RECORD:
        raise ValueError('prepared_runtime_legacy_mutation_unproven')
    return raw


if __name__ == '__main__':
    sandbox_id, agent_id, generation = sys.argv[1:]
    if (os.environ.get('CHRONO_SANDBOX_ID', sandbox_id) != sandbox_id
            or os.environ.get('CHRONO_RUNTIME_GENERATION', generation) != generation):
        raise ValueError('prepared_runtime_environment_changed')
    root = Path(os.environ.get('CHRONO_RUNTIME_STATE_ROOT', '/var/lib/cma-runtime/' + sandbox_id))
    print(json.dumps(observe(root, sandbox_id, agent_id, int(generation))))
