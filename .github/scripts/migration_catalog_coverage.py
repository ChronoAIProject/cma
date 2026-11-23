#!/usr/bin/env python3
"""Join frozen catalog snapshots to the complete source collection inventory.

An absent collection has no catalog evidence. It is never counted as an empty
document fixture or a passing reader cell. This offline audit does not contact
Mongo, Kubernetes or an application and cannot establish document compatibility.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re

from migration_catalog import STAGES, compare, validate


def require(condition, message):
    if not condition:
        raise ValueError('catalog coverage: ' + message)


def read_bound(path, sha256):
    require(isinstance(sha256, str) and re.fullmatch(r'[a-f0-9]{64}', sha256),
            'full SHA256 required')
    raw = Path(path).read_bytes()
    require(hashlib.sha256(raw).hexdigest() == sha256, 'file hash differs')
    return json.loads(raw)


def source_names(inventory):
    require(isinstance(inventory, dict)
            and inventory.get('schema') == 'cma-reader-writer-source-inventory-v1',
            'source inventory schema differs')
    for role in ('old', 'new'):
        value = inventory.get(role)
        require(isinstance(value, dict) and all(isinstance(value.get(field), str)
                and re.fullmatch(r'[a-f0-9]{40}', value[field])
                for field in ('source', 'tree')), 'immutable source identity missing')
    rows = inventory.get('collections')
    require(isinstance(rows, list) and rows, 'source collections missing')
    names = set()
    for row in rows:
        require(isinstance(row, dict) and isinstance(row.get('name'), str)
                and re.fullmatch(r'[a-z][a-z0-9_]*', row['name'])
                and row['name'] not in names, 'source collection invalid or duplicated')
        require(any(isinstance(row.get(role), dict) for role in ('old', 'new')),
                'collection has no source definition')
        names.add(row['name'])
    return names


def artifacts(evidence, inventory):
    selected = {}
    for role in ('old', 'new'):
        value = evidence.get(role)
        require(isinstance(value, dict)
                and value.get('source') == inventory[role]['source'], 'source binding differs')
        image, config = value.get('image'), value.get('config')
        require(isinstance(image, str)
                and re.fullmatch(r'[^\s@]+@sha256:[a-f0-9]{64}', image)
                and isinstance(config, str) and re.fullmatch(r'sha256:[a-f0-9]{64}', config),
                'immutable artifact identity missing')
        selected[role] = {key: value[key] for key in ('source', 'image', 'config')}
    return selected


def audit(inventory, evidence, directory):
    names = source_names(inventory)
    require(isinstance(evidence, dict)
            and evidence.get('schema') == 'cma-migration-kind-evidence-v1',
            'campaign schema differs')
    selected = artifacts(evidence, inventory)
    receipts = evidence.get('catalogs')
    require(isinstance(receipts, list), 'catalog receipts missing')
    directory = Path(directory).resolve()
    snapshots, bindings = {}, {}
    for receipt in receipts:
        require(isinstance(receipt, dict), 'invalid catalog receipt')
        stage = receipt.get('stage')
        require(stage in STAGES and stage not in snapshots, 'stage invalid or duplicated')
        role = 'old' if stage in ('old-write', 'old-read-new-write') else 'new'
        artifact = selected[role]
        require(all(receipt.get(key) == artifact[key] for key in ('source', 'image')),
                'catalog artifact differs')
        pod = receipt.get('serving_pod')
        require(isinstance(pod, dict) and isinstance(pod.get('uid'), str) and pod['uid']
                and pod.get('image_id') == artifact['config'], 'serving pod config differs')
        filename = receipt.get('path')
        require(isinstance(filename, str) and filename == 'catalog-' + stage + '.private.json',
                'catalog path differs')
        path = directory / filename
        require(path.resolve().parent == directory and not path.is_symlink(),
                'catalog path escapes campaign')
        snapshot = read_bound(path, receipt.get('sha256'))
        rows = validate(snapshot)
        require(type(receipt.get('collections')) is int and receipt['collections'] == len(rows),
                'catalog count differs')
        snapshots[stage] = snapshot
        bindings[stage] = {'path': filename, 'sha256': receipt['sha256'],
                           'source': artifact['source'], 'image': artifact['image'],
                           'config': artifact['config'], 'pod_uid': pod['uid']}
    rows_by_stage = {stage: validate(snapshot) for stage, snapshot in snapshots.items()}
    observed = set().union(*(set(rows) for rows in rows_by_stage.values()))
    comparisons = []
    for previous, stage in zip(STAGES, STAGES[1:]):
        # A later snapshot cannot fill an earlier failed or missing stage.
        comparisons.append({'from_stage': previous, 'to_stage': stage,
            **(compare(snapshots[previous], snapshots[stage])
               if previous in snapshots and stage in snapshots else {'outcome': 'NOT_CAPTURED'})})
    cells = []
    for name in sorted(names | observed):
        stages = {}
        for stage in STAGES:
            row = rows_by_stage.get(stage, {}).get(name)
            status = ('NOT_CAPTURED' if stage not in snapshots else
                      'ABSENT_IN_SNAPSHOT' if row is None else 'OBSERVED')
            stages[stage] = {'status': status}
            if row is not None:
                definition = json.dumps(row, sort_keys=True, separators=(',', ':')).encode()
                stages[stage].update(definition_sha256=hashlib.sha256(definition).hexdigest(),
                                     indexes=len(row['indexes']))
        cells.append({'collection': name, 'in_source_inventory': name in names, 'stages': stages})
    return {'schema': 'cma-migration-catalog-coverage-v1',
            'scope': 'Catalog metadata only; no document reads, writes or boot verdict',
            'application_compatibility': 'NOT_ASSESSED',
            'campaign_outcome': evidence.get('outcome'), 'artifacts': selected,
            'source_trees': {role: inventory[role]['tree'] for role in ('old', 'new')},
            'bindings': bindings, 'comparisons': comparisons, 'collections': cells,
            'summary': {'source_collections': len(names),
                        'observed_source_collections': len(names & observed),
                        'unobserved_source_collections': sorted(names - observed),
                        'unexpected_collections': sorted(observed - names),
                        'missing_stages': [stage for stage in STAGES if stage not in snapshots]}}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--inventory', type=Path, required=True)
    parser.add_argument('--inventory-sha256', required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--evidence-sha256', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args(argv)
    inventory = read_bound(args.inventory, args.inventory_sha256)
    evidence = read_bound(args.evidence, args.evidence_sha256)
    result = audit(inventory, evidence, args.evidence.parent)
    result['input_bindings'] = {
        'inventory_sha256': args.inventory_sha256, 'evidence_sha256': args.evidence_sha256}
    # Retain old audits verbatim when a campaign or the source inventory changes.
    with args.output.open('x') as output:
        output.write(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'sha256': hashlib.sha256(args.output.read_bytes()).hexdigest(),
                      'summary': result['summary'], 'application_compatibility': 'NOT_ASSESSED'}))


if __name__ == '__main__':
    main()
