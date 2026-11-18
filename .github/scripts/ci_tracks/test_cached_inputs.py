"""Memoization must preserve source selection and observe every tree change."""

import unittest

from docker_inputs import ignored, matches
from image_inputs import image_inputs
import test_images


class CachedPatternTests(unittest.TestCase):
    def test_directory_recursive_and_literal_boundaries(self):
        cases = [
            ('api/crates/cma/src/lib.rs', 'api', True),
            ('api2/crates/cma/src/lib.rs', 'api', False),
            ('api/crates/cma/src/lib.rs', 'api/*/lib.rs', False),
            ('api/crates/cma/src/lib.rs', 'api/**/lib.rs', True),
            ('api/lib.rs', 'api/**/lib.rs', True),
            ('api/crates/a/src/lib.rs', 'api/crates/[a-c]', True),
            ('api/crates/d/src/lib.rs', 'api/crates/[a-c]', False),
            ('web/file.test.ts/child', 'web/*.test.ts', True),
            ('web/file+name.ts', 'web/file+name.ts', True),
            ('web/fileename.ts', 'web/file+name.ts', False),
            ('anything/nested', '.', True),
        ]
        for _ in range(2):
            for path, pattern, expected in cases:
                with self.subTest(path=path, pattern=pattern):
                    self.assertEqual(matches(path, pattern), expected)

    def test_ignore_order_and_new_rule_text_remain_authoritative(self):
        path = 'web/src/fixture.test.ts'
        exclude = '*.test.ts'
        include = '!web/src/fixture.test.ts'
        self.assertTrue(ignored(path, exclude))
        self.assertFalse(ignored(path, exclude + '\n' + include))
        self.assertTrue(ignored(path, include + '\n' + exclude))
        self.assertFalse(ignored(path, '# comment\n.\n'))
        self.assertTrue(ignored('a/ignored/b.txt', 'ignored'))
        self.assertFalse(ignored('a/ignored/b.txt', '!ignored'))

    def test_invalid_rule_cannot_reuse_a_previous_match(self):
        self.assertTrue(matches('web/a.ts', 'web/*'))
        for _ in range(2):
            with self.assertRaises(ValueError):
                matches('web/a.ts', 'web/[')


class SnapshotCacheTests(unittest.TestCase):
    setUp = test_images.ImageTests.setUp
    write = test_images.ImageTests.write
    selected = test_images.ImageTests.selected
    pin_recipes = test_images.ImageTests.pin_recipes

    def test_ignore_edits_recompute_available_inputs(self):
        path = 'web/src/main.tsx'
        self.assertEqual(self.selected(path), ['cma-web'])
        self.write('.dockerignore', path + '\n')
        self.assertEqual(self.selected(path), [])
        self.write('.dockerignore', path + '\n!' + path + '\n')
        self.assertEqual(self.selected(path), ['cma-web'])

    def test_embedded_input_edits_recompute_analysis(self):
        path = 'api/crates/layer4-runtime/README.md'
        source = 'api/crates/layer4-runtime/src/lib.rs'
        self.write(path, 'embedded product data')
        self.assertEqual(self.selected(path), [])
        self.write(source, 'const TEXT: &str = include_str!("../README.md");')
        self.assertEqual(self.selected(path), ['cma-runtime', 'cma-runtime-omarchy'])
        self.write(source, 'pub fn main() {}')
        self.assertEqual(self.selected(path), [])

    def test_returned_sets_do_not_mutate_a_future_analysis(self):
        first = image_inputs(self.root, self.workspace)
        first['cma-web'].clear()
        second = image_inputs(self.root, self.workspace)
        self.assertIn('web/src/main.tsx', second['cma-web'])


if __name__ == '__main__':
    unittest.main()
