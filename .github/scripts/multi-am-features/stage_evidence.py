"""Summarize only typed scheduler fields; raw errors remain private."""
from collections import Counter
import hashlib
import json
import re

TARGET = 'agent_scheduler_failure'
PHASES = {'discovery', 'reconciliation', 'execution', 'publication'}
FAILURES = {'conflict_unclassified', 'store_unavailable', 'stored_data_invalid',
            'domain_refusal', 'task_panicked', 'task_cancelled'}
PUBLICATION_FAILURES = {'known_aborted_cas', 'owner_authority_refused',
                        'work_lease_changed', 'unknown_commit'}


def fields(line):
    # Kubernetes --timestamps adds one prefix before JSON or the text formatter.
    line = line.strip()
    if re.match(r'^\d{4}-\d{2}-\d{2}T\S+ ', line):
        line = line.split(' ', 1)[1]
    if line.startswith('{'):
        try:
            event = json.loads(line)
        except (ValueError, RecursionError):
            return None
        if not isinstance(event, dict) or event.get('target') != TARGET:
            return None
        value = event.get('fields')
        if not isinstance(value, dict):
            return {}
        # Keep raw messages, identities and caller spans inside the private log.
        return {name: value[name] for name in ['phase', 'failure'] if name in value}
    if re.search(r'\b' + TARGET + r':', line) is None:
        return None
    value = {}
    for name in ['phase', 'failure']:
        match = re.search(r'(?:^|\s)' + name + r'=(?:"([a-z_]+)"|([a-z_]+))(?:\s|$)', line)
        if match:
            value[name] = match[1] or match[2]
    return value


def summarize(directory):
    counts = Counter()
    files = []
    rejected = 0
    # Only the final fixed AM captures: repeated failure snapshots do not inflate
    # counts. Truncated logs are still samples, never a total incident rate.
    for am in ['a', 'b']:
        path = directory / ('am-' + am + '-0.full.private.log')
        if not path.is_file() or path.is_symlink():
            continue
        raw = path.read_bytes()
        files.append({'am': am, 'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw)})
        for line in raw.decode(errors='replace').splitlines():
            value = fields(line)
            if value is None:
                continue
            phase, failure = value.get('phase'), value.get('failure')
            allowed = PUBLICATION_FAILURES if phase == 'publication' else FAILURES
            # External log values may be arrays or objects. Reject them before
            # set membership so one malformed event cannot abort the summary.
            if (not isinstance(phase, str) or not isinstance(failure, str)
                    or phase not in PHASES or failure not in allowed):
                rejected += 1
                continue
            counts[am, phase, failure] += 1
    return {'schema': 'multi-am-stage-evidence-v1', 'scope': 'final_bounded_log_samples',
            'classification': 'emitted_typed_scheduler_fields_only',
            'root_cause_inferred': False, 'complete_history': False,
            'files': files, 'unrecognized_scheduler_events': rejected,
            'occurrences': [{'am': am, 'phase': phase, 'failure': failure, 'count': count}
                            for (am, phase, failure), count in sorted(counts.items())]}
