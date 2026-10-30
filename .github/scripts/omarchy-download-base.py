"""Retry transport failures while preserving the pinned base artifact identity."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import zipfile

assert os.environ.get('GITHUB_ACTIONS') == 'true'
assert os.environ.get('GITHUB_REPOSITORY') == 'ChronoAIProject/cma'
endpoint = 'repos/ChronoAIProject/cma/actions/artifacts/12955388554'
metadata = json.loads(subprocess.check_output(['gh', 'api', endpoint]))
assert metadata['workflow_run']['id'] == 41005657911
# This pull-request workflow tested the merge revision recorded inside the
# export; GitHub's artifact envelope records the PR branch head separately.
assert metadata['workflow_run']['head_sha'] == '87c381d64556a4471ab2ee773a2812b700e88d21'
assert not metadata['expired']
digest = '032a7876df1b63de0e0b64bcc7cbf5586bccba4ae355241114ac36d9f249a3aa'
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
        # The original native export consists of these two regular files.
        required = {'metadata.json', 'cma-runtime-desktop-linux-amd64.docker.tar.gz'}
        assert set(source.namelist()) == required
        for member in required:
            source.extract(member, destination)
# omarchy-base.py then verifies the image archive, successful original workflow,
# source labels, platform and runtime contract before loading/tagging it.
