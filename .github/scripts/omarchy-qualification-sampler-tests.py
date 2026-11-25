#!/usr/bin/env python3
import runpy
import unittest
from pathlib import Path

sampler = runpy.run_path(str(Path(__file__).with_name("omarchy-qualification-sampler.py")))
summarize = sampler["summarize"]


class SamplerTests(unittest.TestCase):
    def test_rejects_one_sample_for_a_ten_minute_interval(self):
        with self.assertRaisesRegex(ValueError, "at least two"):
            summarize(100, 700, [(100, 1000, 2000, 2000)])

    def test_rejects_a_gap_even_with_complete_start_and_end(self):
        with self.assertRaisesRegex(ValueError, "internal gap"):
            summarize(100, 700, [(100, 1000, 2000, 2000), (700, 1100, 2000, 1900)])

    def test_rejects_a_sampler_lost_before_the_end(self):
        with self.assertRaisesRegex(ValueError, "start and end"):
            summarize(100, 110, [(100, 1000, 2000, 2000), (102, 1100, 1900, 1800)])

    def test_complete_interval_reports_peak_and_minimum(self):
        self.assertEqual(
            summarize(101, 105, [
                (100, 1000, 3300, 4500), (102, 1200, 3100, 4600),
                (104, 1100, 3500, 4000), (106, 1050, 3400, 4200),
            ]),
            (4, 1200, 3100, 4000),
        )


if __name__ == "__main__":
    unittest.main()
