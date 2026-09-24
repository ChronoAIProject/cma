#!/usr/bin/env python3
"""Render reviewed Mermaid to static SVG (mermaid-cli 11.17.0), or check freshness.

Install the pinned CLI outside the repository, then pass --mmdc PATH and
--puppeteer-config PATH when Chromium is installed separately. Runtime pages
need neither the CLI nor browser JavaScript.
"""
import argparse
import hashlib
import json
import re
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DESTINATION = ROOT / 'api/crates/cmaeg/assets/docs'
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--check', action='store_true')
parser.add_argument('--mmdc', default='mmdc')
parser.add_argument('--puppeteer-config')
args = parser.parse_args()
rows = []
for document in ['CMAEG_Protocol.md', 'CMAEG_Architecture.md']:
    contents = (ROOT / 'docs' / document).read_text()
    for index, match in enumerate(re.finditer(r'```mermaid\n(.*?)\n```', contents, re.S)):
        source = match.group(1).strip()
        name = ('protocol' if 'Protocol' in document else 'architecture') + '-' + str(index)
        svg = DESTINATION / (name + '.svg')
        if not args.check:
            with tempfile.TemporaryDirectory(prefix='cmaeg-diagram-') as directory:
                path = Path(directory) / 'source.mmd'
                path.write_text(source)
                command = [args.mmdc, '-i', str(path), '-o', str(svg), '-c',
                           str(DESTINATION / 'mermaid.json'), '-b', 'transparent',
                           '-w', '1500', '-I', 'cmaeg-' + name]
                if args.puppeteer_config:
                    command += ['-p', args.puppeteer_config]
                subprocess.run(command, check=True)
        if not svg.exists():
            raise SystemExit('Missing diagram: ' + name)
        rows.append({'source': document, 'index': index,
                     'sha256': hashlib.sha256(source.encode()).hexdigest(), 'name': name})
manifest = DESTINATION / 'manifest.json'
rust = '// Reviewed Mermaid diagrams rendered to static SVG; no browser JavaScript.\n'
rust += 'pub(super) const DIAGRAMS: &[(&str, &str, &str)] = &[\n'
for row in rows:
    rust += '    ("' + row['sha256'] + '", "' + row['name'] + '", include_str!("../../../assets/docs/' + row['name'] + '.svg")),\n'
rust += '];\n'
if args.check:
    if json.loads(manifest.read_text()) != rows:
        raise SystemExit('Stale diagrams: regenerate after editing Mermaid sources')
else:
    manifest.write_text(json.dumps(rows, indent=2) + '\n')
    (ROOT / 'api/crates/cmaeg/src/http/docs/diagrams.rs').write_text(rust)
print('Verified ' + str(len(rows)) + ' gateway diagrams')
