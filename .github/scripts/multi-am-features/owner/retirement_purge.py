"""Advance only an owned disposable Sandbox past its retention deadline."""
import json
from pathlib import Path
import re
from common import require_feature_fixture
from publication_evidence import agent_digest, manager_log_lines


def retirement_purge(self, agentId=None, sandboxId=None, operationId=None, admissions=None, arm=False,
                     titleDigest=None, advanceTitle=False, refusedLeaseSha256=None):
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
    def valid_digest(value):
        return isinstance(value, str) and bool(re.fullmatch(r'[a-f0-9]{64}', value))

    if (type(advanceTitle) is not bool or advanceTitle != (refusedLeaseSha256 is not None)
            or (advanceTitle and (arm or not valid_digest(refusedLeaseSha256) or titleDigest is None))
            or (titleDigest is not None and (len(admissions) != 1 or not valid_digest(titleDigest)))):
        raise ValueError('retirement_purge_title_fixture_scope_required')
    scope = {'agentId': agentId, 'sandboxId': sandboxId, 'operationId': operationId,
             'admissions': admissions, 'arm': arm}
    if titleDigest is not None:
        scope.update(titleDigest=titleDigest, advanceTitle=advanceTitle, refusedLeaseSha256=refusedLeaseSha256)
    scope = json.dumps(scope)
    return self.mongo(f'const scope = {scope};\n' + Path(__file__).with_suffix('.js').read_text())


def retirement_title_deferral(self, agentId=None, sandboxId=None):
    require_feature_fixture(self)
    digest = agent_digest(agentId)
    sandbox_digest = agent_digest(sandboxId)
    observations = {}
    sandbox_token = re.compile(r'(?<!\S)sandbox_id=' + re.escape(sandboxId) + r'(?=\s|$)')
    agent_hash = re.compile(r'(?<!\S)agent_sha256="?'+digest+r'"?(?=\s|$)')
    sandbox_hash = re.compile(r'(?<!\S)sandbox_sha256="?'+sandbox_digest+r'"?(?=\s|$)')
    lease_hash = re.compile(r'(?<!\S)lease_sha256="?([a-f0-9]{64})"?(?=\s|$)')
    for manager in ('a', 'b'):
        lines = list(manager_log_lines(self, manager, '3m'))
        deferrals = [line for line in lines
                     if 'sandbox purge deferred: automatic title provider effect is within its claim deadline' in line
                     and agent_hash.search(line) and sandbox_hash.search(line)]
        observations[manager] = {
            'title_deferrals': len(deferrals),
            'refused_lease_digests': [match[1] for line in deferrals if (match := lease_hash.search(line))],
            'refused_purge_attempts': sum('Sandbox work attempt completed' in line and 'step=Purge ' in line
                                          and 'published=false' in line and bool(sandbox_token.search(line))
                                          for line in lines),
        }
    return observations
