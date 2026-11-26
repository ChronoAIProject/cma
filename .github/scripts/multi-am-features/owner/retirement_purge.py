"""Advance only an owned disposable Sandbox past its retention deadline."""
import json
from pathlib import Path
import re


def retirement_purge(self, agentId=None, sandboxId=None, operationId=None, arm=False):
    run_id = self.ready.get('run_id', '')
    if (self.ready.get('fixture') != 'features' or self.client.namespace != 'cma-native-am'
            or not re.fullmatch(r'cma-ci-[0-9a-f]{12}', run_id) or self.ready.get('context') != 'kind-' + run_id
            or not re.fullmatch(r'https?://127\.0\.0\.1:[0-9]+', self.ready.get('server', ''))
            or type(arm) is not bool or any(not isinstance(value, str) or not re.fullmatch(
                r'[-A-Za-z0-9_]{1,160}', value) for value in (agentId, sandboxId, operationId))):
        raise ValueError('retirement_purge_fixture_scope_required')
    scope = json.dumps({'agentId': agentId, 'sandboxId': sandboxId, 'operationId': operationId, 'arm': arm})
    return self.mongo(f'const scope = {scope};\n' + Path(__file__).with_suffix('.js').read_text())
