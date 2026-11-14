#!/usr/bin/env python3
"""Scheduled 9m/Zig API comparison: 20 minutes, five reads, memory and latency.

This launcher starts local services. The RCA execution owner supplies exact
candidate bindings only after the build and resource handoffs are complete.
"""
import argparse
import concurrent.futures
import hashlib
import json
import math
from pathlib import Path
import re
import signal
import threading
import time

import hierarchy_seed
import light
import pair
import repeat

SECONDS = 1200
ROUTES = ('/livez', '/ready', '/openapi.json', hierarchy_seed.SANDBOX_ROUTE,
          hierarchy_seed.WORKSPACE_ROUTE)
BASE = pair.SOURCE
PATCH = '1075a8c1374e878adcf168c8cd135f9c0fe9d5eb'
CANDIDATE = 'dfc2e7ba55b337b169ec9de749224d95cc6a6631'
LOCK_SHA256 = '46e5cef4a4a4af47c953e421e4242af115935268fd0c6694ae9c1f4ac20f722c'


def nearest_rank(values, probability):
    if not values:
        return None
    return sorted(values)[math.ceil(probability * len(values)) - 1]


def latency(rows):
    durations = [r['duration_seconds'] * 1000 for r in rows if r['outcome'] == 'completed']
    return {'successful_samples': len(durations), 'attempted_samples': len(rows),
            'failed_samples': len(rows) - len(durations),
            'p50_ms': nearest_rank(durations, .50), 'p95_ms': nearest_rank(durations, .95),
            'method': 'nearest rank ceil(p*N), full HTTP body, monotonic clock'}


def request_loop(config, start, output, stop):
    rows = []
    with (output / 'requests.jsonl').open('x') as journal:
        for index in range(SECONDS):
            if stop.wait(max(0, start + index - time.monotonic())):
                break
            route = ROUTES[index % len(ROUTES)]
            row = {'index': index, 'route': route, 'utc': pair.utc(),
                   'lag_seconds': max(0, time.monotonic() - (start + index))}
            began = time.monotonic()
            try:
                body = repeat.get(config['origin'], route)
                row['duration_seconds'] = time.monotonic() - began
                row['http_status'] = 200
                hierarchy_seed.validate_body(route, body)
                row.update(outcome='completed', bytes=len(body))
                del body
            except Exception as error:
                row.setdefault('duration_seconds', time.monotonic() - began)
                row.update(outcome='failed', failure_type=type(error).__name__)
                if hasattr(error, 'status'):
                    row['http_status'] = error.status
            rows.append(row)
            journal.write(json.dumps(row) + '\n')
            journal.flush()
    completed = sum(row['outcome'] == 'completed' for row in rows)
    return {'planned': SECONDS, 'attempted': len(rows), 'completed': completed,
            'failed': len(rows) - completed, 'not_attempted': SECONDS - len(rows),
            'completed_within_window': len(rows) == SECONDS and time.monotonic() - start <= SECONDS,
            'request_loop_ended_elapsed_seconds': time.monotonic() - start,
            'late_over_one_second': sum(row['lag_seconds'] > 1 for row in rows),
            'bytes': sum(row.get('bytes', 0) for row in rows), 'aggregate_latency': latency(rows),
            'routes': {route: {'planned': SECONDS // len(ROUTES),
                                **latency([row for row in rows if row['route'] == route])}
                       for route in ROUTES}}


def fit(rows, field):
    return repeat.fit([dict(row, rss_kib=row[field]) for row in rows])


def gates(variants):
    candidate, control = variants['mimalloc'], variants['zig']
    complete = all(v.get('complete') for v in variants.values())
    ratios = {}
    for percentile in ('p50_ms', 'p95_ms'):
        numerator = candidate.get('counts', {}).get('aggregate_latency', {}).get(percentile)
        denominator = control.get('counts', {}).get('aggregate_latency', {}).get(percentile)
        ratios[percentile] = numerator / denominator if numerator is not None and denominator else None
    anon = candidate.get('full_anonymous_fit')
    return {'complete_pair': complete,
            'candidate_anonymous_under_1': bool(complete and anon and anon['mib_per_minute'] < 1),
            'aggregate_latency_ratio': ratios,
            'aggregate_p50_p95_within_10_percent': bool(complete and all(v is not None and v <= 1.10 for v in ratios.values()))}


def measure(configs, output):
    observers = {kind: repeat.Observer(config) for kind, config in configs.items()}
    rows = {kind: [] for kind in configs}
    errors = {kind: [] for kind in configs}
    stop = threading.Event()
    result = {'routes': list(ROUTES), 'planned_seconds': SECONDS, 'requests_per_second_each': 1,
              'samples_planned': 41, 'sample_interval_seconds': 30, 'variants': {}}

    def sample(kind, phase, index):
        row = observers[kind].sample(phase, index)
        rows[kind].append(row)
        with (output / kind / 'samples.jsonl').open('a') as journal:
            journal.write(json.dumps(row) + '\n')

    for kind in configs:
        sample(kind, 'initial', 0)
    start = time.monotonic()
    result['loop_started_utc'] = pair.utc()
    (output / 'loop-start.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'event': 'loop_started', 'utc': result['loop_started_utc'], 'seconds': SECONDS}), flush=True)
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            futures = {kind: pool.submit(request_loop, config, start, output / kind, stop)
                       for kind, config in configs.items()}
            try:
                for index in range(1, 41):
                    time.sleep(max(0, start + index * 30 - time.monotonic()))
                    for kind in configs:
                        try:
                            sample(kind, 'loop', index)
                        except Exception as error:
                            errors[kind].append({'utc': pair.utc(), 'index': index, 'failure_type': type(error).__name__})
                result['loop_ended_utc'] = pair.utc()
                result['actual_loop_seconds'] = time.monotonic() - start
                stop.set()
                for kind, future in futures.items():
                    try:
                        result['variants'][kind] = {'counts': future.result()}
                    except Exception as error:
                        errors[kind].append({'utc': pair.utc(), 'failure_type': type(error).__name__})
            finally:
                stop.set()
        time.sleep(12)
        for kind in configs:
            try:
                sample(kind, 'quiescent', 41)
            except Exception as error:
                errors[kind].append({'utc': pair.utc(), 'index': 41, 'failure_type': type(error).__name__})
    finally:
        for kind in configs:
            observed = rows[kind]
            loop = [row for row in observed if row['phase'] != 'quiescent']
            variant = result['variants'].setdefault(kind, {})
            counts = variant.get('counts', {})
            variant.update(observation_errors=errors[kind], process_identity=observers[kind].identity,
                           image=configs[kind]['docker_image_id'], source=configs[kind]['source'],
                           full_rss_fit=fit(loop, 'rss_kib'), full_anonymous_fit=fit(loop, 'anonymous_kib'),
                           after_process_age_10min_rss_fit=fit([r for r in loop if r['process_age_seconds'] >= 600], 'rss_kib'),
                           after_process_age_10min_anonymous_fit=fit([r for r in loop if r['process_age_seconds'] >= 600], 'anonymous_kib'))
            if observed:
                variant.update(rss_kib=light.describe(loop, 'rss_kib'), anonymous_kib=light.describe(loop, 'anonymous_kib'),
                               final_observation=observed[-1],
                               gauge_ranges={key: [min(r[key] for r in observed), max(r[key] for r in observed)]
                                             for key in ('threads', 'cma_runtime_alive_tasks', 'cma_public_streams_active',
                                                         'cma_runtime_global_queue_depth', 'cma_http_in_flight', 'cma_runtime_workers')})
            variant['complete'] = bool(counts.get('completed') == SECONDS and not counts.get('failed')
                                       and not counts.get('not_attempted') and counts.get('completed_within_window') and len(loop) == 41
                                       and result.get('actual_loop_seconds', 0) >= SECONDS and not errors[kind]
                                       and observed[-1]['phase'] == 'quiescent'
                                       and observed[-1]['cma_public_streams_active'] == 0
                                       and observed[-1]['cma_http_in_flight'] == 0)
        result['gates'] = gates(result['variants'])
        result['verdict'] = 'PENDING_FIXTURE_POSTFLIGHT_AND_CLEANUP'
        (output / 'measurement.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def candidate_image(config):
    if config.get('application_base') != BASE or config.get('allocator_commit') != PATCH:
        raise ValueError('exact batch9 base and allocator-only commit are required')
    expected = config['candidate']
    for key in ('docker_image_id', 'manifest', 'config'):
        if not re.fullmatch(r'sha256:[0-9a-f]{64}', expected[key]):
            raise ValueError('full candidate digest bindings required')
    if expected['source'] != CANDIDATE:
        raise ValueError('exact batch9 plus allocator candidate source required')
    actual = json.loads(pair.docker('image', 'inspect', '--format',
                        '{"id":{{json .Id}},"descriptor":{{json .Descriptor}},"architecture":{{json .Architecture}},"source":{{json (index .Config.Labels "org.opencontainers.image.revision")}}}',
                        expected['docker_image_id']))
    if (actual['id'] != expected['docker_image_id'] or actual['architecture'] != 'amd64'
            or actual['source'] != expected['source'] or actual['descriptor']['digest'] != expected['manifest']
            or actual['descriptor']['annotations']['config.digest'] != expected['config']):
        raise ValueError('candidate image/source/config binding mismatch')
    return actual


def run(config_path, output):
    config = json.loads(config_path.read_text())
    bindings = {'mimalloc': candidate_image(config), 'zig': pair.image(pair.OLD)}
    contract = {'grant': 'LEAD 15 Nov 01:03 mimalloc qualification; debugger02 owns scheduled execution',
                'routes': list(ROUTES), 'planned_seconds': SECONDS, 'requests_per_second_each': 1,
                'requests_each_route_each_image': 240, 'sample_interval_seconds': 30,
                'candidate_anonymous_slope_mib_min_strictly_below': 1,
                'aggregate_p50_p95_ratio_at_most': 1.10, 'percentile_method': 'nearest_rank',
                'application_base': BASE, 'allocator_commit': PATCH,
                'candidate_source': CANDIDATE, 'candidate_lock_sha256': LOCK_SHA256,
                'binding_config_sha256': hashlib.sha256(config_path.read_bytes()).hexdigest(),
                'resource_context': config.get('resource_context', 'Owner must record scheduled shared-host context.')}
    try:
        light.run(output, image_bindings=bindings, fixture=hierarchy_seed.Fixture(), measurement=measure, contract=contract)
    finally:
        campaign_path = output / 'campaign.json'
        if campaign_path.exists():
            campaign = json.loads(campaign_path.read_text())
            measurement = campaign.get('measurement', {})
            gate = measurement.get('gates', {})
            passed = (campaign['outcome'] == 'completed' and campaign.get('fixture_before') and campaign.get('fixture_after')
                      and gate.get('complete_pair') and gate.get('candidate_anonymous_under_1')
                      and gate.get('aggregate_p50_p95_within_10_percent'))
            verdict = {'verdict': 'PASS' if passed else ('FAIL' if campaign['outcome'] == 'completed' else 'INCOMPLETE'),
                       'gates': gate, 'campaign_outcome': campaign['outcome'], 'images': campaign['images'],
                       'contract': contract, 'ended_utc': campaign['ended_utc'], 'cleanup': campaign['cleanup']}
            (output / 'qualification.json').write_text(json.dumps(verdict, indent=2) + '\n')
    return json.loads((output / 'qualification.json').read_text())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bindings', type=Path, help='nonsecret exact build-owner image/source/config bindings')
    parser.add_argument('output', type=Path, help='new owner evidence directory; must not already exist')
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        result = run(args.bindings.resolve(), args.output.resolve())
        print(json.dumps(result))
        raise SystemExit(0 if result['verdict'] == 'PASS' else 1)
    except Exception as error:
        print(json.dumps({'verdict': 'INCOMPLETE', 'failure_type': type(error).__name__}))
        raise SystemExit(2)
