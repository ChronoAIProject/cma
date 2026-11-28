from contextlib import closing
import hashlib
import json
from pathlib import Path
import sqlite3
import sys
from tempfile import TemporaryDirectory
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent / 'owner'))
from prepared_runtime_ledger import observe


class PreparedRuntimeLedgerTests(unittest.TestCase):
    agent = 'synthetic-agent'
    sandbox = 'synthetic-sandbox'
    operations = (
        ('worktree:workspace:wt-1:add:v1', 'add_worktree', 0),
        (f'agent:{agent}:home:create:v1', 'create_agent_home', 0),
        (f'agent:{agent}:open:v1', 'open_agent', 1),
    )

    def record(self, operation, kind, scope):
        return json.dumps({'sandbox_id': self.sandbox, 'operation_id': operation,
                           'runtime_generation': scope or None, 'request_sha256': 'f' * 64,
                           'subject': {'kind': kind, 'agent_id': self.agent}, 'status': 'completed',
                           'receipt': {'operation_id': operation, 'request_sha256': 'f' * 64}}).encode()

    def test_legacy_evidence_is_exact_and_reads_the_open_from_the_original_generation(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            digests = []
            for operation, kind, scope in self.operations:
                path = root / f'process-generation-{scope}' if scope else root
                path /= 'runtime-mutations'
                path.mkdir(parents=True, exist_ok=True)
                raw = self.record(operation, kind, scope)
                name = hashlib.sha256(operation.encode()).hexdigest() + '.json'
                (path / name).write_bytes(raw)
                digests.append(hashlib.sha256(raw).hexdigest())
            self.assertEqual(observe(root, self.sandbox, self.agent, 1),
                             {'format': 'legacy', 'receipts': digests})
            with self.assertRaisesRegex(ValueError, 'prepared_runtime_mutation_unbound'):
                observe(root, 'foreign-sandbox', self.agent, 1)

    def test_indexed_evidence_refuses_duplicate_scope_even_on_the_exact_operation_id(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'mutation-ledger-v2.current.json').touch()
            indexed = root / 'mutation-ledger-v2'
            indexed.mkdir()
            with closing(sqlite3.connect(indexed / 'ledger.sqlite3')) as db:
                db.execute('CREATE TABLE operations (scope BLOB, operation_id TEXT, status INT, record BLOB)')
                for operation, kind, scope in self.operations:
                    db.execute('INSERT INTO operations VALUES (?, ?, 2, ?)',
                               (scope.to_bytes(8, 'big'), operation, self.record(operation, kind, scope)))
                db.commit()
                result = observe(root, self.sandbox, self.agent, 1)
                self.assertEqual(result['format'], 'indexed')
                self.assertEqual(len(set(result['receipts'])), 3)
                db.execute('INSERT INTO operations SELECT ?, operation_id, status, record FROM operations WHERE scope=?',
                           ((2).to_bytes(8, 'big'), (1).to_bytes(8, 'big')))
                db.commit()
            with self.assertRaisesRegex(ValueError, 'prepared_runtime_indexed_mutation_unproven'):
                observe(root, self.sandbox, self.agent, 1)
