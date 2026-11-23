"""Publication timing reports preserve bounded, sanitized evidence."""
import io
import json
import unittest
from unittest.mock import Mock

from staging_cd_build_progress import summarize
from staging_cd_timing import Timings


class TimingTests(unittest.TestCase):
    def test_runtime_scan_duration_and_cache_counts_stay_sanitized(self):
        timings = Timings(clock=Mock(side_effect=[0, 3, 4, 11]))
        with timings.stage("build"):
            pass
        with timings.stage("scan"):
            timings.detector_cache(dict(hits=4, misses=1, withheld=0, secret="private"))
        for invalid in (None, {}, dict(hits=True, misses=0, withheld=0),
                        dict(hits=0, misses=-1, withheld=0), dict(hits=10**10, misses=0, withheld=0)):
            timings.detector_cache(invalid)
        output = io.StringIO()
        timings.emit(output, scan_only=True)
        self.assertEqual([json.loads(line) for line in output.getvalue().splitlines()], [
            {"event": "publication-detector-cache", "hits": 4, "misses": 1, "withheld": 0},
            {"event": "publication-stage", "stage": "scan", "seconds": 7, "result": "success"},
        ])

    def test_failure_retains_only_fixed_stage_and_duration(self):
        timings = Timings(clock=Mock(side_effect=[10, 12.5]))
        with self.assertRaises(ValueError), timings.stage('scan'):
            raise ValueError('synthetic private response')
        output = io.StringIO()
        timings.emit(output)
        row = json.loads(output.getvalue())
        self.assertEqual(row, {'event': 'publication-stage', 'stage': 'scan', 'seconds': 2.5, 'result': 'failure'})
        self.assertNotIn('private response', output.getvalue())
        with self.assertRaises(ValueError), timings.stage('unknown unsafe name'):
            pass

    def test_progress_categories_never_emit_commands_or_sum_overlap(self):
        records = []
        for n, name, start, end in [(1, 'RUN cargo build --release synthetic-private-argument', 0, 10),
                                    (2, 'RUN cargo build --release', 5, 15),
                                    (3, 'exporting layers', 16, 18), (4, 'RUN private-value', 0, 5),
                                    (5, 'importing cache manifest from private-path', 0, 2),
                                    (6, 'exporting cache to private-target', 17, 20)]:
            records.append({'id': 'sha256:' + str(n) * 64, 'name': name,
                            'started': f'2026-01-01T00:00:{start:02d}Z',
                            'completed': f'2026-01-01T00:00:{end:02d}Z'})
        output = summarize(b'\n'.join(json.dumps(r).encode() for r in records))
        self.assertEqual(output['category_seconds'], {'rust-compile': 15, 'other-build': 5,
                         'archive-export': 2, 'cache-import': 2, 'cache-export': 3,
                         'dependency-compile': 0, 'go-compile': 0, 'media-compile': 0})
        self.assertNotIn('private', json.dumps(output))
        self.assertFalse(summarize(b'not json')['complete'])

    def test_other_native_builds_are_not_counted_as_rust(self):
        rows = [
            {'id': str(index), 'name': name, 'started': '2026-01-01T00:00:00Z',
             'completed': '2026-01-01T00:00:04Z'}
            for index, name in enumerate([
                '[dependencies 2/2] RUN cargo chef cook --release private-value',
                '[secret-scanner-build 5/5] RUN go build private-value',
                '[ffmpeg-build 4/4] RUN ./configure --prefix=private-value; make',
            ])
        ]
        result = summarize(b'\n'.join(json.dumps(row).encode() for row in rows))
        self.assertEqual(result['category_seconds']['rust-compile'], 0)
        for name in ('dependency-compile', 'go-compile', 'media-compile'):
            self.assertEqual(result['category_seconds'][name], 4)
        self.assertNotIn('private-value', json.dumps(result))

    def test_a_failed_build_names_only_the_stage_and_step_where_it_stopped(self):
        rows = [
            {'id': '1', 'name': '[runtime-base 3/12] RUN apt-get install private-value',
             'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:09Z',
             'error': 'exit code: 100 private-error'},
            {'id': '2', 'name': '[codex 2/4] RUN curl private-value',
             'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:10Z',
             'error': 'context canceled'},
            {'id': '3', 'name': '[internal] load metadata for docker.io/library/private:tag',
             'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:01Z'},
            {'id': '4', 'name': '[build 9/9] RUN cargo build private-value',
             'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:02Z'},
        ]
        result = summarize(b'\n'.join(json.dumps(row).encode() for row in rows))
        self.assertEqual(result['failed_steps'], ['runtime-base 3/12', 'codex 2/4'])
        self.assertNotIn('private', json.dumps(result))
        metadata = [{'id': '5', 'name': '[internal] load metadata for ghcr.io/private/image:tag',
                     'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:03Z',
                     'error': 'failed to resolve: private'},
                    {'id': '6', 'name': 'unnamed private vertex', 'error': 'private'}]
        result = summarize(b'\n'.join(json.dumps(row).encode() for row in metadata))
        self.assertEqual(result['failed_steps'], ['other', 'base-image-metadata'])
        self.assertNotIn('private', json.dumps(result))
        self.assertNotIn('failed_steps', summarize(b'\n'.join(json.dumps(row).encode() for row in rows[2:])))
        # BuildKit pads the position to the width of the total in longer stages.
        padded = [{'id': '7', 'name': '[media-self-test  7/11] RUN private-value', 'error': 'private'},
                  {'id': '8', 'name': '[build   9/120] RUN private-value', 'error': 'private',
                   'completed': '2026-01-01T00:00:01Z'}]
        result = summarize(b'\n'.join(json.dumps(row).encode() for row in padded))
        self.assertEqual(result['failed_steps'], ['media-self-test 7/11', 'build 9/120'])
        self.assertNotIn('private', json.dumps(result))

    def test_an_ignored_cache_export_failure_is_cache_evidence_not_a_failed_step(self):
        # BuildKit keeps an ignore-error export failure on its exporter vertex,
        # with the cache service's refusal as go-actions-cache relays it.
        budget = ('error writing layer blob: failed to reserve cache: You have reached your configured budget, '
                  'your cache is now read only to prevent additional charges. private-value')
        step = {'id': '1', 'name': '[build 3/9] RUN cargo build private-value',
                'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:09Z'}
        image = {'id': '2', 'name': 'exporting to oci image format',
                 'started': '2026-01-01T00:00:09Z', 'completed': '2026-01-01T00:00:12Z'}
        export = {'id': '3', 'name': 'exporting to GitHub Actions Cache',
                  'started': '2026-01-01T00:00:12Z', 'completed': '2026-01-01T00:00:22Z'}

        def summary(*rows):
            return summarize(b'\n'.join(json.dumps({'vertexes': [row]}).encode() for row in rows))
        result = summary(step, image, dict(export, error=budget))
        self.assertEqual(result['cache_export_error'], 'read-only')
        self.assertNotIn('failed_steps', result)
        self.assertEqual(result['category_seconds']['cache-export'], 10)
        self.assertEqual(result['category_seconds']['archive-export'], 3)
        for withheld in ('private', 'budget', 'reserve'):
            self.assertNotIn(withheld, json.dumps(result))
        # Another export failure is not explained, and a real step failure still reports.
        result = summary(dict(step, error='exit code: 101'), dict(export, error='maximum timeout reached private'))
        self.assertEqual((result['cache_export_error'], result['failed_steps']), ('failed', ['build 3/9']))
        self.assertNotIn('private', json.dumps(result))
        self.assertNotIn('cache_export_error', summary(step, image, export))

    def test_a_failed_test_stage_keeps_its_names_beside_a_failed_cache_export(self):
        # The export vertex is cache evidence, never a second failed step or a
        # source of test names; the failed test stage still names its tests.
        import staging_cd_test_failures_tests as failures
        raw = failures.progress(
            (1, failures.QUALIFICATION, 'process did not complete successfully: private', {1: failures.SPEC}),
            (2, 'exporting to GitHub Actions Cache',
             'failed to reserve cache: You have reached your configured budget, private', {}))
        result = summarize(raw, dockerfile=failures.RUNTIME)
        self.assertEqual(result['failed_steps'], [failures.STEP])
        self.assertEqual(result['failed_tests'], failures.SPEC_NAMES)
        self.assertEqual(result['cache_export_error'], 'read-only')
        self.assertNotIn('private', json.dumps(result))


if __name__ == '__main__':
    unittest.main()
