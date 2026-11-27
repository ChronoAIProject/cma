"""Advance only an owned disposable Sandbox past its retention deadline."""
import json
from pathlib import Path
import re
from common import require_feature_fixture


def retirement_purge(self, agentId=None, sandboxId=None, operationId=None, admissions=None, arm=False):
    require_feature_fixture(self)

    def valid_id(value):
        return isinstance(value, str) and bool(re.fullmatch(r'[-A-Za-z0-9_]{1,160}', value))

    if (type(arm) is not bool or not all(map(valid_id, (agentId, sandboxId, operationId)))
            or not isinstance(admissions, list) or not 1 <= len(admissions) <= 32
            or not all(isinstance(row, dict) and row.keys() == {'agentId', 'operationId'}
                       and all(map(valid_id, row.values())) for row in admissions)):
        raise ValueError('retirement_purge_fixture_scope_required')
    if (len({row['agentId'] for row in admissions}) != len(admissions)
            or len({row['operationId'] for row in admissions}) != len(admissions)
            or {'agentId': agentId, 'operationId': operationId} not in admissions):
        raise ValueError('retirement_purge_fixture_scope_required')
    scope = json.dumps({'agentId': agentId, 'sandboxId': sandboxId, 'operationId': operationId,
                        'admissions': admissions, 'arm': arm})
    return self.mongo(f'const scope = {scope};\n' + Path(__file__).with_suffix('.js').read_text())
