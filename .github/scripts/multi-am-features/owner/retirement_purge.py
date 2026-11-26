"""Advance only an owned disposable Sandbox past its retention deadline."""
import json
from pathlib import Path
import re
from common import require_feature_fixture


def retirement_purge(self, agentId=None, sandboxId=None, operationId=None, arm=False):
    require_feature_fixture(self)
    if type(arm) is not bool or any(not isinstance(value, str) or not re.fullmatch(
            r'[-A-Za-z0-9_]{1,160}', value) for value in (agentId, sandboxId, operationId)):
        raise ValueError('retirement_purge_fixture_scope_required')
    scope = json.dumps({'agentId': agentId, 'sandboxId': sandboxId, 'operationId': operationId, 'arm': arm})
    return self.mongo(f'const scope = {scope};\n' + Path(__file__).with_suffix('.js').read_text())
