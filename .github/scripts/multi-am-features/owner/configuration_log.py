"""Read bounded, payload-free configuration receipts from disposable MongoDB."""
import hashlib
import json
import re
from pathlib import Path


def configuration_log(self, agentId=None, after=None):
    if (not isinstance(agentId, str) or not re.fullmatch(r'[-A-Za-z0-9_]{1,160}', agentId)
            or (after is not None and (type(after) is not int or not 0 <= after < 2**53))):
        raise ValueError('configuration_log_lineage_required')
    scope = json.dumps({'agent': agentId, 'after': after})
    query = Path(__file__).with_name('configuration_log.js').read_text()
    return self.mongo(f'const scope = {scope};\n' + query)


def configuration_dispatch(self, agentId=None, sequence=None):
    if (not isinstance(agentId, str) or not re.fullmatch(r'[-A-Za-z0-9_]{1,160}', agentId)
            or type(sequence) is not int or not 0 < sequence < 2**53):
        raise ValueError('configuration_dispatch_lineage_required')
    digest = hashlib.sha256(agentId.encode()).hexdigest()
    observations = {}
    for manager in ('a', 'b'):
        raw = self.client.k(['logs', f'am-{manager}-0', '--since=3m', '--limit-bytes=1048576'], timeout=5)
        counts = {'started': 0, 'confirmed': 0}
        for entry in raw.splitlines():
            line = re.sub(r'\x1b\[[0-9;]*m', '', entry)
            if 'layer2_core::sandbox::settings_dispatch:' not in line:
                continue
            fields = dict(re.findall(r'(?<!\S)(agent_sha256|sequence|stage)="?([a-z0-9_]+)"?(?=\s|$)', line))
            if fields.get('agent_sha256') == digest and fields.get('sequence') == str(sequence):
                stage = fields.get('stage')
                if stage in counts:
                    counts[stage] += 1
        observations[manager] = counts
    return observations
