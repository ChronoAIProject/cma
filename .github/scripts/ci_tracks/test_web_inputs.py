"""Pin the web reference search to the original whole-corpus search."""

from pathlib import Path
import random
import tempfile
import unittest

from docker_inputs import matches
from web_inputs import public_directory, web_inputs

EXCLUSIONS = ['e2e/**', '**/tests/**', '**/__tests__/**', '**/test/**', '**/*.test.*', '**/*.spec.*', 'README.md', 'README']


def reference(root, files, exclusions):
    # The original algorithm, kept verbatim: every round rescans every retained
    # text, joined by b'\n', once per remaining test.
    candidates = {p for p in files if p.startswith('web/')}
    public = public_directory(root)
    assets = {p for p in candidates if public and (p == public or p.startswith(public + '/'))}
    source_suffixes = {'.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs', '.json'}
    tests = {p for p in candidates - assets if
             (p.startswith(('web/e2e/', 'web/src/', 'web/build/')) and Path(p).suffix in source_suffixes
              and any(matches(p[4:], pattern) for pattern in exclusions)) or p in {'web/README', 'web/README.md'}}
    text_suffixes = source_suffixes | {'.css', '.scss', '.sass', '.less', '.html', '.svg'}
    retained = candidates - tests
    while True:
        text = b'\n'.join((root / p).read_text(errors='replace').encode()
                           for p in retained if Path(p).suffix in text_suffixes)
        referenced = {p for p in tests - retained
                      if Path(p).stem.encode(errors='surrogatepass') in text}
        if not referenced:
            return retained
        retained.update(referenced)


class WebInputEquivalence(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    def write(self, path, data, root=None):
        file = (root or self.root) / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_bytes(data if isinstance(data, bytes) else data.encode())

    def inputs(self, root=None):
        root = root or self.root
        files = {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file()}
        actual = web_inputs(root, set(files), EXCLUSIONS)
        expected = reference(root, set(files), EXCLUSIONS)
        # Same members, and the same set operations, hence the same order.
        self.assertEqual(list(actual), list(expected))
        return actual

    def test_checked_in_vite_config_models_public_directory(self):
        repository = Path(__file__).resolve().parents[3]
        self.assertEqual(public_directory(repository), 'web/public')

    def test_chain_found_only_in_files_added_by_later_rounds(self):
        self.write('web/src/main.tsx', 'import "./a.test";')
        self.write('web/src/a.test.ts', 'import "./b.spec";')
        self.write('web/src/b.spec.ts', '@import "c.test";')
        self.write('web/src/c.test.ts', 'export const leaf = 1;')
        self.write('web/src/d.test.ts', 'import "./a.test";')
        retained = self.inputs()
        self.assertTrue({'web/src/a.test.ts', 'web/src/b.spec.ts', 'web/src/c.test.ts'} <= retained)
        self.assertNotIn('web/src/d.test.ts', retained)

    def test_shared_stems_share_one_answer(self):
        self.write('web/src/main.tsx', 'import "./x/client.test";')
        for path in ['web/src/x/client.test.ts', 'web/src/y/client.test.tsx', 'web/e2e/client.test.js']:
            self.write(path, 'import "./status";')
        self.write('web/e2e/status.ts', '')
        self.write('web/src/test/status.json', '{}')
        retained = self.inputs()
        self.assertTrue({'web/src/y/client.test.tsx', 'web/e2e/client.test.js', 'web/src/test/status.json'} <= retained)

    def test_matches_are_raw_substrings_of_decoded_text(self):
        self.write('web/src/main.tsx', 'xa.testy; [id].test; b . test; \xe9.test')
        self.write('web/src/style.css', b'.x{background:url(\xff.spec)}\n')
        for name in ['a.test', '[id].test', 'b.test', '\xe9.test', '�.spec', 'z.test']:
            self.write(f'web/src/{name}.ts', '')
        self.write('web/src/logo.png', 'z.test')
        retained = self.inputs()
        self.assertTrue({'web/src/a.test.ts', 'web/src/[id].test.ts', 'web/src/\xe9.test.ts', 'web/src/�.spec.ts'} <= retained)
        self.assertFalse({'web/src/b.test.ts', 'web/src/z.test.ts'} & retained)

    def test_stems_containing_newlines_still_match_across_the_join(self):
        self.write('web/src/a\nb.test.ts', '')
        self.write('web/src/first.ts', 'b.test = a')
        self.assertNotIn('web/src/a\nb.test.ts', self.inputs())
        # Every retained text starts with "b.test" and ends with "a", so any
        # join order puts "a\nb.test" across a separator.
        self.write('web/src/second.ts', 'b.test = a')
        self.assertIn('web/src/a\nb.test.ts', self.inputs())
        self.write('web/src/second.ts', 'one')
        self.write('web/src/third.css', b'a\r\nb.test')
        self.assertIn('web/src/a\nb.test.ts', self.inputs())

    def test_newline_stems_span_files_retained_in_different_rounds(self):
        self.write('web/src/a\nb.test.ts', '')
        self.write('web/src/main.tsx', 'b.test "./mid.test" a')
        self.write('web/src/mid.test.ts', 'b.test a')
        self.assertIn('web/src/a\nb.test.ts', self.inputs())

    def test_newline_stems_see_files_in_the_original_join_order(self):
        # Whether "a\nb.test" spans "a" and "b.test" depends on the set order
        # the original search joined them in; each tree is another chance to
        # differ from a search that joins them in any other order.
        for case in range(16):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.write('web/src/a\nb.test.ts', '', root)
                self.write(f'web/src/{case}-end.ts', 'a', root)
                self.write(f'web/src/{case}-start.ts', 'b.test', root)
                self.inputs(root)

    def test_public_assets_and_readmes(self):
        self.write('web/public/data.test.json', '{"see": "hidden.test"}')
        self.write('web/src/hidden.test.ts', 'docs: README')
        self.write('web/README.md', 'guide')
        self.write('web/README', 'guide')
        self.write('web/src/other.test.ts', '')
        retained = self.inputs()
        self.assertTrue({'web/public/data.test.json', 'web/src/hidden.test.ts', 'web/README.md', 'web/README'} <= retained)
        self.assertNotIn('web/src/other.test.ts', retained)

    def test_random_trees_match_the_reference(self):
        rng = random.Random(2175)
        names = ['a', 'b', 'client', 'a\nb', '[id]', '\xe9', 'x-y']
        directories = ['web/src/', 'web/src/x/', 'web/src/test/', 'web/e2e/', 'web/build/', 'web/public/']
        suffixes = ['.test.ts', '.spec.tsx', '.ts', '.json', '.css']
        fragments = [*names, '.test', '.spec', 'README', '\n', '\r\n', ' ', '"./', b'\xff', b'\xed\xa0\x80', '\xe9', '\U0001f388']
        retained = ['web/src/main.tsx', 'web/index.html', 'web/src/style.css', 'web/src/logo.png']
        for case in range(150):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                paths = retained + ['web/README.md', 'web/README']
                paths += [rng.choice(directories) + rng.choice(names) + rng.choice(suffixes) for _ in range(rng.randint(2, 12))]
                for path in paths:
                    parts = [rng.choice(fragments) for _ in range(rng.randint(0, 8))]
                    self.write(path, b''.join(p if isinstance(p, bytes) else p.encode() for p in parts), root)
                self.inputs(root)


if __name__ == '__main__':
    unittest.main()
