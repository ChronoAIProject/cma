"""Advance only an owned disposable Sandbox past its retention deadline."""
import json
from pathlib import Path
import re
from common import require_feature_fixture
from publication_evidence import agent_digest, manager_log_lines


def retirement_purge(self, agentId=None, sandboxId=None, operationId=None, admissions=None, arm=False,
                     titleDigest=None, advanceTitle=False):
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
    if (type(advanceTitle) is not bool or (advanceTitle and (arm or titleDigest is None))
            or (titleDigest is not None and (len(admissions) != 1 or not isinstance(titleDigest, str)
                or not re.fullmatch(r'[a-f0-9]{64}', titleDigest)))):
        raise ValueError('retirement_purge_title_fixture_scope_required')
    scope = {'agentId': agentId, 'sandboxId': sandboxId, 'operationId': operationId,
             'admissions': admissions, 'arm': arm}
    if titleDigest is not None:
        scope.update(titleDigest=titleDigest, advanceTitle=advanceTitle)
    scope = json.dumps(scope)
    return self.mongo(f'const scope = {scope};\n' + Path(__file__).with_suffix('.js').read_text())


def retirement_title_deferral(self, agentId=None, sandboxId=None):
    require_feature_fixture(self)
    digest = agent_digest(agentId)
    agent_digest(sandboxId)
    observations = {}
    sandbox_token = re.compile(r'(?<!\S)sandbox_id=' + re.escape(sandboxId) + r'(?=\s|$)')
    for manager in ('a', 'b'):
        lines = list(manager_log_lines(self, manager, '3m'))
        observations[manager] = {
            'title_deferrals': sum('sandbox purge deferred: automatic title provider effect is within its claim deadline' in line
                                   and f'agent_sha256={digest}' in line for line in lines),
            'refused_purge_attempts': sum('Sandbox work attempt completed' in line and 'step=Purge ' in line
                                          and 'published=false' in line and bool(sandbox_token.search(line))
                                          for line in lines),
        }
    return observations
