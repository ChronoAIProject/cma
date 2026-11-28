"""Allowlisted command outcomes; never publish command arguments or raw output."""
import hashlib
import re

INITIALIZATION_WAITS = {
    'role_not_ready', 'authenticated_mongo_primary_not_ready',
    'runtime_visible_public_api_unreachable', 'provider_not_ready',
    'two_members_not_ready', 'client_deadline', 'infrastructure_deadline',
    'am_pod_not_ready_a', 'am_pod_not_ready_b', 'am_not_ready_a', 'am_not_ready_b',
    'provenance_progress_command_timeout',
    'forward_not_ready_am-a-0', 'forward_not_ready_am-b-0',
    'forward_not_ready_provider', 'forward_not_ready_identity',
}


def initialization_timeout(error):
    # Only labels supplied by the fixture's fixed waits are public. An arbitrary
    # socket timeout or exception text may contain URLs or caller data.
    if isinstance(error, TimeoutError) and str(error) in INITIALIZATION_WAITS:
        return str(error)
    return None


def suite_error_kind(output):
    match = re.search(r'^(TypeError|ReferenceError|SyntaxError|RangeError|AssertionError|Error):',
                      output, re.MULTILINE)
    return match[1] if match else 'unclassified'


def suite_error_frame(output, suite_root):
    """Publish a source location from Node stderr without paths or exception text."""
    suite_root = suite_root.resolve()
    script_origin = re.escape(suite_root.as_uri()) + '/'
    # Match the fixture's source directories. npm stores dependencies under
    # node_modules, which can also contain .mjs files below the fixture root.
    source_module = r'((?:(?:cma|features|owner|sec01)/)?[a-z][a-z0-9_-]*\.mjs)'
    location = re.compile(
        r'^(?:[ \t]+at (?:async )?(?:[^\n]+? \()?)?' + script_origin + source_module
        + r':([1-9][0-9]{0,4})(?::[1-9][0-9]{0,4})?\)?(?: \{)?$',
        re.MULTILINE)
    for match in location.finditer(output):
        module, line = match.groups()
        path = suite_root / module
        if path.resolve() == path and path.is_file():
            return {'module': module, 'line': int(line)}
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
        'api_forbidden': (b'error from server (forbidden)',),
        'api_invalid_resource': (b'error from server (invalid)', b' is invalid: '),
        'admission_webhook_unreachable': (b'failed calling webhook', b'failed to call webhook'),
        'admission_webhook_denied': (b'denied the request',),
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
        'durable_store_startup_failed': (b'durable stores are not usable:',),
        'document_storage_startup_failed': (b'document storage binding:',),
    }
    lower = output.lower()
    return {'stage': stage,
            'program': program if program in ['docker', 'kubectl', 'kind'] else 'other',
            'returncode': returncode, 'elapsed_ms': round(elapsed * 1000, 3),
            'timed_out': timed_out,
            'signals': [name for name, needles in signals.items() if any(n in lower for n in needles)],
            'output_sha256': hashlib.sha256(output).hexdigest()}
