"""Keep the same-source Mongo controls independently accounted for in CI."""
import json
from pathlib import Path
import re
import sys


def controls():
    names = Path(__file__).with_name('startup-controls.txt').read_text().splitlines()
    if len(names) != 51 or len(set(names)) != len(names) or not all(
            re.fullmatch(r'[a-z0-9_]+(?:::[a-z0-9_]+)+', name) for name in names):
        raise ValueError('lifecycle publication control inventory is invalid')
    return names


def filter_expression(names):
    return 'package(=cma) and (' + ' or '.join(f'test(={name})' for name in names) + ')'


def require_exact_listing(names, listing):
    selected = [case for suite in listing['rust-suites'].values()
                for case, details in suite.get('testcases', {}).items() if details['filter-match']['status'] == 'matches']
    if sorted(selected) != sorted(names):
        raise ValueError('lifecycle publication control inventory differs from the Rust archive')
    return len(selected)


if __name__ == '__main__':
    requested = controls()
    if sys.argv[1:] == ['--filter']:
        print(filter_expression(requested))
    elif sys.argv[1:] == ['--check-listing']:
        count = require_exact_listing(requested, json.load(sys.stdin))
        print(f'Lifecycle publication source controls selected {count}/{len(requested)} exact tests.')
    else:
        raise SystemExit('expected --filter or --check-listing')
