#!/usr/bin/env python3
"""One bounded, idempotent D4 reconciliation pass. Default is read-only."""
import argparse
import json
import os
from pathlib import Path
import ssl
import sys
import time
from urllib.error import HTTPError
from urllib.parse import quote, urlencode
from urllib.request import Request, build_opener, ProxyHandler, HTTPSHandler, HTTPRedirectHandler

from policy import (CLASS, binding, digest, eligible, policy_ref, request_id,
                    require, validate_class, validate_disk, validate_policy)


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('http_redirect_refused')


class Client:
    def __init__(self):
        self.deadline = time.monotonic() + 240
        sa = Path('/var/run/secrets/kubernetes.io/serviceaccount')
        self.token_path = sa / 'token'
        self.kube = build_opener(ProxyHandler({}), NoRedirect(), HTTPSHandler(
            context=ssl.create_default_context(cafile=str(sa / 'ca.crt'))))
        self.cloud = build_opener(ProxyHandler({}), NoRedirect())

    def exchange(self, opener, url, headers, body=None):
        require(time.monotonic() < self.deadline, 'pass_deadline')
        data = None if body is None else json.dumps(body).encode()
        req = Request(url, data=data, headers={**headers, 'Content-Type': 'application/json'})
        try:
            with opener.open(req, timeout=min(15, max(1, self.deadline - time.monotonic()))) as response:
                raw = response.read(8 * 1024 * 1024 + 1)
                require(len(raw) <= 8 * 1024 * 1024, 'response_too_large')
                return json.loads(raw)
        except HTTPError as error:
            # Never echo a response body, token, request URL or header.
            raise ValueError(f'http_{error.code}') from None

    def kget(self, path):
        require(path.startswith(('/api/v1/', '/apis/storage.k8s.io/v1/')), 'kubernetes_path_refused')
        return self.exchange(self.kube, 'https://kubernetes.default.svc' + path,
                             {'Authorization': 'Bearer ' + self.token_path.read_text().strip()})

    def compute(self, path, body=None):
        require(path.startswith('projects/'), 'compute_path_refused')
        access = self.exchange(self.cloud, 'http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token',
                               {'Metadata-Flavor': 'Google'})
        return self.exchange(self.cloud, 'https://compute.googleapis.com/compute/v1/' + path,
                             {'Authorization': 'Bearer ' + access['access_token']}, body)


def claim_path(config, pvc):
    return '/api/v1/namespaces/' + quote(config['namespace'], safe='') + '/persistentvolumeclaims/' + quote(pvc['metadata']['name'], safe='')


def current(client, pvc, storage_class, config):
    fresh = client.kget(claim_path(config, pvc))
    require(fresh['metadata']['uid'] == pvc['metadata']['uid'], 'pvc_replaced')
    name = fresh['spec'].get('volumeName')
    require(name, 'volume_not_bound')
    pv = client.kget('/api/v1/persistentvolumes/' + quote(name, safe=''))
    handle = binding(fresh, pv, storage_class, config)
    disk = client.compute(handle)
    policies = validate_disk(disk, handle, fresh, config)
    return fresh, pv, handle, disk, policies


def reconcile_one(client, pvc, storage_class, config, apply=False):
    fresh, pv, handle, disk, policies = current(client, pvc, storage_class, config)
    expected = policy_ref(config)
    if expected in policies:
        return 'attached'
    # Preserve all existing policies. A different schedule/placement policy is
    # an explicit operator decision, never something this job removes.
    require(not policies, 'foreign_policy_present')
    if not apply:
        return 'would_attach'
    newer, new_pv, new_handle, new_disk, new_policies = current(client, fresh, storage_class, config)
    require((handle, disk['id'], pv['metadata']['uid']) ==
            (new_handle, new_disk['id'], new_pv['metadata']['uid']), 'attachment_identity_changed')
    if expected in new_policies:
        return 'attached'
    require(not new_policies, 'foreign_policy_present')
    operation = client.compute(handle + '/addResourcePolicies?' + urlencode({'requestId': request_id(disk, config)}),
                               {'resourcePolicies': [expected]})
    scope = '/'.join(handle.split('/')[:4])
    require(operation.get('name') and '/' not in operation['name'], 'operation_identity_missing')
    for _ in range(8):
        require(not operation.get('error'), 'attach_operation_failed')
        if operation.get('status') == 'DONE':
            observed = client.compute(handle)
            require(observed['id'] == disk['id'], 'disk_replaced_after_attach')
            require(expected in validate_disk(observed, handle, newer, config), 'attach_readback_missing')
            return 'attached'
        time.sleep(2)
        operation = client.compute(scope + '/operations/' + operation['name'])
    raise ValueError('attach_pending')


def run(client, config, apply=False):
    storage_class = client.kget('/apis/storage.k8s.io/v1/storageclasses/' + CLASS)
    validate_class(storage_class, config)
    validate_policy(client.compute(policy_ref(config)), config)
    claims, cursor, cursors = [], '', set()
    # Finish pagination before mutating. A failed/looping page cannot certify
    # complete coverage, and no kubernetes writes are needed for bookkeeping.
    for _ in range(100):
        page = client.kget('/api/v1/namespaces/' + quote(config['namespace'], safe='') +
                           '/persistentvolumeclaims?' + urlencode({'limit': 200, 'continue': cursor}))
        claims.extend(p for p in page['items'] if eligible(p))
        cursor = page.get('metadata', {}).get('continue', '')
        if not cursor:
            break
        require(cursor not in cursors, 'pagination_loop')
        cursors.add(cursor)
    else:
        raise ValueError('pagination_limit')
    require(len(claims) < 900, 'schedule_capacity_guard')
    failed = False
    for pvc in claims:
        try:
            result = reconcile_one(client, pvc, storage_class, config, apply)
        except Exception as error:
            result = error.args[0] if isinstance(error, ValueError) and len(error.args) == 1 else 'attachment_unavailable'
            if not isinstance(result, str) or not result.replace('_', '').isalnum():
                result = 'attachment_unavailable'
            failed = True
        print(json.dumps({'pvc_sha256': digest(pvc['metadata']['uid']), 'result': result}), flush=True)
    print(json.dumps({'selected': len(claims), 'failed': failed, 'mode': 'apply' if apply else 'observe'}), flush=True)
    return 1 if failed else 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--config', required=True)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    # Manifest defaults have no --apply, are suspended, and need independent
    # explicit activation. Runtime only grants the sole attach operation.
    with open(args.config) as source:
        config = json.load(source)
    return run(Client(), config, args.apply)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Exception:
        print(json.dumps({'result': 'reconciliation_failed'}), flush=True)
        sys.exit(1)
