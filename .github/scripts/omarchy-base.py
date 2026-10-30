"""Verify the exact successful CI artifact before reusing its runtime binaries."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1])
run = json.loads(subprocess.check_output(['gh', 'api', 'repos/ChronoAIProject/cma/actions/runs/41005657911']))
assert run['conclusion'] == 'success' and run['run_attempt'] == 1
assert run['repository']['full_name'] == 'ChronoAIProject/cma'
meta = json.loads((root / 'metadata.json').read_text())
assert meta['sourceRevision'] == '43bf9e131431efa77d977af7ba724d8111efdd5a'
assert meta['component'] == 'runtime-desktop'
assert meta['platform'] == 'linux/amd64' and meta['runtimeContract'] == '3'
assert meta['workflow']['runId'] == '41005657911'
archive = root / meta['archive']
assert archive.parent == root and archive.stat().st_size == meta['archiveBytes']
with archive.open('rb') as source:
    assert hashlib.file_digest(source, 'sha256').hexdigest() == meta['archiveSha256']
subprocess.run(['docker', 'load', '-i', str(archive)], check=True)
image = json.loads(subprocess.check_output(['docker', 'inspect', meta['imageId']]))[0]
assert image['Config']['Labels']['org.opencontainers.image.revision'] == meta['sourceRevision']
assert image['Config']['Labels']['dev.chrono.cma.runtime-contract'] == '3'
subprocess.run(['docker', 'tag', meta['imageId'], 'cma-omarchy-base:verified'], check=True)
evidence = Path('/tmp/omarchy-evidence')
evidence.mkdir(exist_ok=True)
(evidence / 'base-provenance.json').write_text(json.dumps(meta, indent=2))
archive.unlink()
