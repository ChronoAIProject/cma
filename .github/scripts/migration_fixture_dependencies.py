"""Select exactly one declared dependency overlay for a migration campaign."""
from importlib import import_module


DEPENDENCIES = {
    'bot_fixture': ('migration_bot_provider', 'scoped synthetic Bot gateway/key dependencies'),
    'gateway_fixture': ('migration_gateway_provider', 'scoped gateway-record authority'),
    'rate_fixture': ('migration_rate_fixture', 'one-per-minute disposable input budget'),
    'sandbox_admission_fixture': ('migration_sandbox_fixture', 'read-only runtime authority'),
    'share_fixture': ('migration_share_provider', 'synthetic organization grants'),
    'warm_pool_fixture': ('migration_sandbox_fixture', 'zero-capacity runtime pools'),
    'directory_fixture': ('migration_directory_provider', 'retained directory names'),
}


def add_arguments(parser):
    group = parser.add_mutually_exclusive_group()
    for name, (_, help_text) in DEPENDENCIES.items():
        group.add_argument('--' + name.replace('_', '-'), action='store_true', help=help_text)


def configure(objects, args):
    selected = [name for name in DEPENDENCIES if getattr(args, name, False)]
    if len(selected) > 1:
        raise ValueError('migration dependency overlays are mutually exclusive')
    if not selected:
        return objects
    module, _ = DEPENDENCIES[selected[0]]
    return import_module(module).manifests(objects)
