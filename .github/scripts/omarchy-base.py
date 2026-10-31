"""Verify the retained candidate as a binary donor, not a working desktop."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1])
run = json.loads(subprocess.check_output(['gh', 'api', 'repos/ChronoAIProject/cma/actions/runs/41005657911']))
assert run['conclusion'] == 'success' and run['run_attempt'] == 1
assert run['repository']['full_name'] == 'ChronoAIProject/cma'
original = json.loads((root / 'evidence/base-provenance.json').read_text())
assert original['schema'] == 'cma-ci-native-image-v1'
assert original['sourceRevision'] == '43bf9e131431efa77d977af7ba724d8111efdd5a'
assert original['imageId'] == 'sha256:c812e89b7149b814c728e640a92696097cfe9e2649eed3714e5d6f25bd9da082'
assert original['archiveSha256'] == 'f5ccfd639c9799b5289c44a5bacb52b3821334ab21d88829f2fb46e64b7542c3'
assert original['component'] == 'runtime-desktop'
assert original['workflow']['runId'] == '41005657911'
assert original['platform'] == 'linux/amd64' and original['runtimeContract'] == '3'
meta = json.loads((root / 'metadata.json').read_text())
assert meta['schema'] == 'cma-experimental-image-v1'
assert meta['sourceRevision'] == '3f7bef51e0676abef0fb6e51e701b037bcb0d249'
assert meta['imageId'] == 'sha256:832a39526c7c00e385bdeebc66cb92b943d22bac88ae914d98601092714b1130'
assert meta['component'] == 'runtime-omarchy'
assert meta['platform'] == 'linux/amd64' and meta['runtimeContract'] == '3'
assert meta['workflow']['runId'] == '41100886329'
assert meta['verification']['runtimeContract'] == 'success'
# This exact donor failed desktop acceptance. Preserve that fact explicitly.
assert meta['admission'] == 'blocked' and meta['productionValidated'] is False
contract = json.loads((root / 'evidence/runtime-contract.json').read_text())
assert contract['result'] == 'pass' and contract['cleanup'] is True
assert contract['image'] == meta['imageId'] and contract['revision'] == meta['sourceRevision']
assert len(contract['proof']['checks']) == len(set(contract['proof']['checks'])) == 34
expected = {
    'runtime': '1e7c98a5acd9af3610a62187687b1b0e932ceec61cf902c8c1de137ed2b17c32',
    'gh': 'de7a06e5a17ee90007b7de832de2328dbbb1700f390f3fd04b26d90b803fdf8b',
    'launcher': '61b0194f3bb6534439c8d26a3ed57d0805f84b884588b761795323eeb92fcf70',
    'native': 'c06a67fd8ac76508dc78bd99ce20c9f317b8d568beda59485f3f4134df9962d5',
    'acp': 'ac50122023f18cec80c4ef543dfbcffe8029de4206cb2692c2f5df51341c2c66',
}
assert all(contract['proof']['artifacts'][name] == digest for name, digest in expected.items())
assert meta['archive'] == 'cma-runtime-omarchy-linux-amd64.docker.tar.gz'
archive = root / meta['archive']
assert archive.parent == root and archive.stat().st_size == meta['archiveBytes']
with archive.open('rb') as source:
    assert hashlib.file_digest(source, 'sha256').hexdigest() == meta['archiveSha256']
subprocess.run(['docker', 'load', '-i', str(archive)], check=True)
image = json.loads(subprocess.check_output(['docker', 'inspect', meta['imageId']]))[0]
assert image['Config']['Labels']['org.opencontainers.image.revision'] == meta['sourceRevision']
assert image['Config']['Labels']['dev.chrono.cma.runtime-contract'] == '3'
assert image['Os'] == 'linux' and image['Architecture'] == 'amd64'
subprocess.run(['docker', 'tag', meta['imageId'], 'cma-omarchy-base:verified'], check=True)
evidence = Path('/tmp/omarchy-evidence')
evidence.mkdir(exist_ok=True)
(evidence / 'base-provenance.json').write_text(json.dumps(original, indent=2))
(evidence / 'binary-donor.json').write_text(json.dumps({
    'scope': 'runtime-binaries-only', 'metadata': meta, 'runtimeContract': contract,
}, indent=2))
archive.unlink()
