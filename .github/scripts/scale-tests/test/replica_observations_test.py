"""Censored phase evidence must survive without inventing completion or custody."""
import json
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from replica_observations import phase_observations


class ObservationTests(unittest.TestCase):
    def test_interleaved_replica_headers_retain_failures_and_cohort_clock(self):
        result = phase_observations(
            b"test runner_binary::replicas::test ... replica_stage replicas=2 ownership=true phase=cohort state=start ordinal=0 elapsed_ms=0\n"
            b"replica_stage replicas=2 ownership=true phase=headers state=start ordinal=1 elapsed_ms=10\n"
            b"replica_stage replicas=2 ownership=true phase=headers state=start ordinal=2 elapsed_ms=11\n"
            b"replica_http replicas=2 ownership=true ordinal=2 status=503 elapsed_ms=20\n"
            b"replica_http replicas=2 ownership=true ordinal=1 status=202 elapsed_ms=24\n"
            b"replica_stage replicas=2 ownership=true phase=headers state=end ordinal=1 elapsed_ms=24\n")
        self.assertEqual([row['status'] for row in result['ingress_header_observations']], [503, 202])
        self.assertEqual([row['process_ordinal'] for row in result['ingress_header_observations']], [2, 1])
        self.assertEqual(result['cohort_stage_observations'][0]['cohort_elapsed_ms'], 0)
        self.assertEqual(len(result['cohort_stage_observations']), 4)
        self.assertNotIn('duration', json.dumps(result))

    def test_unapproved_fields_and_invalid_values_do_not_escape(self):
        result = phase_observations(
            b"replica_stage replicas=1 ownership=true phase=private-payload state=start ordinal=0 elapsed_ms=1\n"
            b"replica_stage replicas=1 ownership=true phase=ready state=end ordinal=2 elapsed_ms=1\n"
            b"replica_stage replicas=1 ownership=true phase=cohort state=start ordinal=1 elapsed_ms=1\n"
            b"replica_stage replicas=1 ownership=true phase=cohort state=start ordinal=0 elapsed_ms=-1\n"
            b"replica_stage replicas=1 ownership=true phase=cohort state=start ordinal=0 elapsed_ms=1 private-payload\n"
            b"replica_http replicas=1 ownership=true ordinal=2 status=202 elapsed_ms=1\n"
            b"replica_http replicas=1 ownership=true ordinal=1 status=999 elapsed_ms=1\n")
        self.assertEqual(result, {'cohort_stage_observations': [], 'ingress_header_observations': []})


if __name__ == '__main__':
    unittest.main()
