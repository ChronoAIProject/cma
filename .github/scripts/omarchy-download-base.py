"""Download a pinned retained binary donor; its desktop is not accepted."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import shutil
import tempfile
import time
import zipfile

assert os.environ.get('GITHUB_ACTIONS') == 'true'
assert os.environ.get('GITHUB_REPOSITORY') == 'ChronoAIProject/cma'
endpoint = 'repos/ChronoAIProject/cma/actions/artifacts/12997689773'
metadata = json.loads(subprocess.check_output(['gh', 'api', endpoint]))
assert metadata['workflow_run']['id'] == 41100886329
assert metadata['workflow_run']['head_sha'] == '3f7bef51e0676abef0fb6e51e701b037bcb0d249'
assert not metadata['expired']
digest = '872b260e2c7ef9cfd85d15d1800935373f42d8bb10403b6216efc54caec81f65'
assert metadata['digest'] == 'sha256:' + digest
destination = Path('/tmp/cma-base')
destination.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix='cma-base-transfer-') as directory:
    archive = Path(directory) / 'artifact.zip'
    for attempt in range(3):
        try:
            with archive.open('wb') as target:
                process = subprocess.Popen(['gh', 'api', endpoint + '/zip'], stdout=target)
                started = time.monotonic()
                previous_size, progress_at = 0, started
                try:
                    while process.poll() is None:
                        time.sleep(5)
                        size = archive.stat().st_size
                        now = time.monotonic()
                        if size != previous_size:
                            progress_at = now
                            previous_size = size
                        print('Base artifact downloaded bytes:', size, flush=True)
                        if now - progress_at > 180 or now - started > 900:
                            raise subprocess.TimeoutExpired('base artifact transfer', int(now - started))
                    if process.returncode:
                        raise subprocess.CalledProcessError(process.returncode, 'base artifact transfer')
                finally:
                    if process.poll() is None:
                        process.kill()
                    process.wait()
            with archive.open('rb') as source:
                assert hashlib.file_digest(source, 'sha256').hexdigest() == digest
            break
        except (subprocess.SubprocessError, OSError, AssertionError):
            if attempt == 2:
                raise
            print('Base artifact transport retry', attempt + 1, flush=True)
            time.sleep(2)
    with zipfile.ZipFile(archive) as source:
        # Extract only these literal members; never follow paths or links from
        # the archive. The enclosing ZIP digest pins all retained evidence.
        required = {'metadata.json', 'cma-runtime-omarchy-linux-amd64.docker.tar.gz',
                    'evidence/base-provenance.json', 'evidence/runtime-contract.json'}
        for name in required:
            assert source.namelist().count(name) == 1
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True)
            with source.open(name) as member, target.open('wb') as output:
                shutil.copyfileobj(member, output)
# The original successful desktop archive has expired. omarchy-base.py verifies
# the retained candidate's runtime-only evidence and original base provenance.
