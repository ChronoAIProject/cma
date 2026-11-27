"""Allowlisted command outcomes; never publish command arguments or raw output."""
import hashlib

INITIALIZATION_WAITS = {
    'role_not_ready', 'authenticated_mongo_primary_not_ready',
    'runtime_visible_public_api_unreachable', 'provider_not_ready',
    'two_members_not_ready', 'client_deadline', 'infrastructure_deadline',
    'am_pod_not_ready_a', 'am_pod_not_ready_b', 'am_not_ready_a', 'am_not_ready_b',
    'forward_not_ready_am-a-0', 'forward_not_ready_am-b-0',
    'forward_not_ready_provider', 'forward_not_ready_identity',
}


def initialization_timeout(error):
    # Only labels supplied by the fixture's fixed waits are public. An arbitrary
    # socket timeout or exception text may contain URLs or caller data.
    if isinstance(error, TimeoutError) and str(error) in INITIALIZATION_WAITS:
        return str(error)
    return None


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
        'executable_format': (b'exec format error',),
        'configuration_refused': (b'configuration is not usable:',),
        'application_stopped': (b'cma stopped',),
        'application_listening': (b'cma listening',),
        'stack_overflow': (b'has overflowed its stack', b'fatal runtime error: stack overflow'),
        'segmentation_fault': (b'segmentation fault',),
        'database_unauthorized': (b'not authorized on', b'command failed with code 13'),
        'database_authentication_failed': (b'authentication failed', b'code 18 (authenticationfailed)'),
        'warm_pool_startup_failed': (b'native warm pool startup readiness failed:',),
        'warm_pool_configuration_failed': (b'native warm pool default is not usable:',),
    }
    lower = output.lower()
    return {'stage': stage,
            'program': program if program in ['docker', 'kubectl', 'kind'] else 'other',
            'returncode': returncode, 'elapsed_ms': round(elapsed * 1000, 3),
            'timed_out': timed_out,
            'signals': [name for name, needles in signals.items() if any(n in lower for n in needles)],
            'output_sha256': hashlib.sha256(output).hexdigest()}
