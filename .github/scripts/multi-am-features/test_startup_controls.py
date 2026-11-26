import unittest

from startup_controls import controls, filter_expression, require_exact_listing


class StartupControlsTests(unittest.TestCase):
    def test_exact_names_pass_and_other_tests_are_not_selected(self):
        names = controls()
        expression = filter_expression(names)
        self.assertTrue(expression.startswith('package(=cma) and ('))
        for name in names:
            self.assertEqual(expression.count(f'test(={name})'), 1)
        cases = {name: {'filter-match': {'status': 'matches'}} for name in names}
        cases['unselected::control'] = {'filter-match': {'status': 'mismatch'}}
        listing = {'rust-suites': {'cma::bin/cma': {'testcases': cases}}}
        self.assertEqual(require_exact_listing(names, listing), 5)

    def test_missing_extra_or_duplicate_names_fail_the_control_job(self):
        names = controls()
        for selected in [names[:-1], [*names, 'unexpected::control'], [*names, names[0]]]:
            suites = {str(index): {'testcases': {name: {'filter-match': {'status': 'matches'}}}}
                      for index, name in enumerate(selected)}
            with self.assertRaisesRegex(ValueError, 'inventory differs'):
                require_exact_listing(names, {'rust-suites': suites})
