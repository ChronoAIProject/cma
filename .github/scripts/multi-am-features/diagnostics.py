"""Allowlisted command outcomes; never publish command arguments or raw output."""
import hashlib


def command_outcome(stage, program, returncode, elapsed, output, timed_out=False):
    signals = {
        'disk_full': (b'no space left on device',),
        'image_platform_missing': (b'no matching manifest for',),
        'image_pull_refused': (b'pull access denied', b'failed to authorize'),
        'registry_throttled': (b'toomanyrequests', b'too many requests'),
        'connection_refused': (b'connection refused',),
        'connection_timeout': (b'connection timed out', b'i/o timeout'),
        'resource_mapping_missing': (b'no matches for kind', b'resource mapping not found'),
        'resource_already_exists': (b'alreadyexists', b'already exists'),
        'rollout_timeout': (b'timed out waiting for the condition', b'progress deadline exceeded'),
        'permission_denied': (b'permission denied',),
        'image_not_found': (b'manifest unknown', b'image not found'),
    }
    lower = output.lower()
    return {'stage': stage,
            'program': program if program in ['docker', 'kubectl', 'kind'] else 'other',
            'returncode': returncode, 'elapsed_ms': round(elapsed * 1000, 3),
            'timed_out': timed_out,
            'signals': [name for name, needles in signals.items() if any(n in lower for n in needles)],
            'output_sha256': hashlib.sha256(output).hexdigest()}
