"""Resolve a canonical image identity from the shared public build manifest."""

import argparse
import json
import os
from pathlib import Path

from image_inputs import recipes


def resolve(root, component):
    matches = [(name, value) for name, value in recipes(root).items() if value['component'] == component]
    if len(matches) != 1:
        raise ValueError('unknown or ambiguous image component')
    name, recipe = matches[0]
    return dict(recipe, name=name)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--component', required=True)
    args = parser.parse_args()
    recipe = resolve(Path(__file__).resolve().parents[3], args.component)
    print(json.dumps(recipe, sort_keys=True))
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
            output.write('name=' + recipe['name'] + '\n')


if __name__ == '__main__':
    main()
