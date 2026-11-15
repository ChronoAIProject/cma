"""Keep the overview read-only and the production downscale cadence fixed."""
from pathlib import Path
import re
import shlex

import operations as o
from test_controller import NoExternalCommands, release


class GuardedGuideTests(NoExternalCommands):
    guide = Path(__file__).resolve().parents[3] / 'docs' / 'CMA_Scale_Apply.md'

    def test_overview_shell_examples_only_read_resources(self):
        blocks = re.findall(r'^```(?:bash|sh|shell)\n(.*?)^```',
                            self.guide.read_text(), re.MULTILINE | re.DOTALL)
        self.assertTrue(blocks, 'Keep the read-only resource checks available')
        for block in blocks:
            for line in block.replace('\\\n', ' ').splitlines():
                if not line.strip() or line.lstrip().startswith('#'):
                    continue
                with self.subTest(command=line):
                    # A copied overview command must never bypass stage guards.
                    self.assertNotRegex(line, r'[;|&`<>]|\$\(')
                    words = shlex.split(line)
                    if words[:2] == ['kubectl', '--context']:
                        words = ['k'] + words[3:]
                    self.assertEqual(words[0], 'k')
                    self.assertIn(words[1], ('get', 'top'))

    def test_overview_links_to_each_guarded_stage(self):
        text = self.guide.read_text()
        for name in ('deployment-runbooks', 's1-execution', 's2-execution', 's3-execution'):
            target = f'notes/reviews/2026-11-15/{name}.md'
            with self.subTest(target=target):
                self.assertIn(f']({target})', text)
                self.assertTrue((self.guide.parent / target).is_file())

    def test_both_hpas_hold_above_ninety_seconds_without_widening_period(self):
        for role in ('am', 'eg'):
            for p95 in (0, 90, 90.000001, None, float('nan')):
                with self.subTest(role=role, p95=p95):
                    value = release()
                    value['hpa'][role]['handoff_p95_seconds'] = p95
                    if p95 in (0, 90):
                        op = o.build(f's3-{role}-create', value)
                        down = op['manifest']['spec']['behavior']['scaleDown']
                        self.assertEqual(down['stabilizationWindowSeconds'], 300)
                        self.assertEqual(down['policies'],
                                         [{'type': 'Pods', 'value': 1, 'periodSeconds': 120}])
                    else:
                        with self.assertRaisesRegex(RuntimeError, 'p95'):
                            o.build(f's3-{role}-create', value)
