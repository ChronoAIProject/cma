"""Collect sanitized, Agent-scoped publication diagnostics from both Managers."""
import hashlib
import re


FAILURES = frozenset(('known_aborted_cas', 'owner_authority_refused', 'work_lease_changed', 'unknown_commit'))
STARTUP_STEPS = frozenset(('install_skill', 'wait_for_workspace', 'add_worktree', 'materialize_home',
                           'open_conversation', 'publish_ready', 'publish_connection', 'dispatch_first_message'))
LOG_BOUND_BYTES = 2 * 1024 * 1024
LOG_TARGET = ' agent_scheduler_failure: '
FIELDS = re.compile(r'(?<!\S)(phase|failure|next_step|agent_sha256)="?([a-z0-9_]+)"?(?=\s|$)')


def publication_evidence(fixture, agentId=None):
    if not isinstance(agentId, str) or not re.fullmatch(r'[-A-Za-z0-9_]{1,160}', agentId):
        raise ValueError('publication_evidence_lineage_required')
    digest = hashlib.sha256(agentId.encode()).hexdigest()
    observations = {}
    for manager in ('a', 'b'):
        raw = fixture.client.k(['logs', f'am-{manager}-0', '--since=12m',
                                f'--limit-bytes={LOG_BOUND_BYTES}'], timeout=8)
        if len(raw.encode()) >= LOG_BOUND_BYTES:
            raise RuntimeError('publication_evidence_log_bound')
        counts = {}
        for entry in raw.splitlines():
            line = re.sub(r'\x1b\[[0-9;]*m', '', entry)
            if LOG_TARGET not in line:
                continue
            fields = dict(FIELDS.findall(line.split(LOG_TARGET, 1)[1]))
            if fields.get('agent_sha256') != digest or fields.get('phase') != 'publication':
                continue
            failure, step = fields.get('failure'), fields.get('next_step')
            if failure not in FAILURES or not step:
                raise RuntimeError('publication_evidence_invalid_diagnostic')
            if step not in STARTUP_STEPS:
                continue
            by_step = counts.setdefault(failure, {})
            by_step[step] = by_step.get(step, 0) + 1
        observations[manager] = counts
    return observations
