"""Isolated native web build/scan/transport proof; never publish to a registry."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request

from image_secret_gate import checksum, scan_archive
from staging_cd_build_progress import summarize

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / '.github/scripts/image-secret-scanner'))
from image_secrets.archive import ImageArchive
from image_secrets.common import Budget

PREFIXES = ('usr/share/nginx/current/assets/', 'usr/share/nginx/html/assets/')


def command(args, **options):
    result = subprocess.run(args, capture_output=True, check=False, timeout=900, **options)
    if result.returncode:
        raise ValueError('benchmark-command-failed')
    return result.stdout


def asset_inventory(archive):
    """Validate actual tar bytes and link targets without filesystem extraction."""
    entries, layers = {}, []
    with tempfile.TemporaryDirectory(prefix='web-layer-inventory-') as temp:
        with ImageArchive(archive, Path(temp), Budget(2 * 1024**3, 100000)) as image:
            _, images = image.images()
            if len(images) != 1:
                raise ValueError('expected-one-image')
            for name, digest in zip(images[0]['layers'], images[0]['diff_ids']):
                path = image.layer(name, digest)
                count = payload = links = 0
                with tarfile.open(path, 'r:') as layer:
                    for member in layer:
                        if not member.name.startswith(PREFIXES) or member.isdir():
                            continue
                        if member.isfile():
                            data = layer.extractfile(member).read()
                            entries[member.name] = ('file', hashlib.sha256(data).hexdigest())
                            payload += len(data)
                        elif member.islnk():
                            entries[member.name] = ('link', member.linkname)
                            links += 1
                        else:
                            raise ValueError('asset-must-be-regular-or-hardlink')
                        count += 1
                layers.append({'diff_id': digest, 'asset_entries': count,
                               'asset_payload_bytes': payload, 'hardlinks': links})
                path.unlink()
    contents = []
    for prefix in PREFIXES:
        resolved = {}
        for path, (kind, value) in entries.items():
            if not path.startswith(prefix):
                continue
            if kind == 'link':
                # The paired payload must be present, regular and inside the
                # other exact asset root; no outside target or link chain.
                target = entries.get(value)
                if not target or target[0] != 'file' or not value.startswith(PREFIXES):
                    raise ValueError('invalid-asset-hardlink')
                value = target[1]
            resolved[path[len(prefix):]] = value
        contents.append(resolved)
    if not contents[0] or contents[0] != contents[1]:
        raise ValueError('asset-paths-differ')
    return {'layers': layers, 'asset_files_per_path': len(contents[0]),
            'asset_content_sha256': hashlib.sha256(json.dumps(contents[0], sort_keys=True).encode()).hexdigest(),
            'asset_payload_bytes': sum(x['asset_payload_bytes'] for x in layers),
            'hardlinks': sum(x['hardlinks'] for x in layers)}, contents[0]


def smoke(archive, directory, assets, expected_sha):
    tag = 'cma-web-release-benchmark:' + directory.name
    container = None
    retained = directory / 'retained'
    retained.mkdir(mode=0o755)
    retained.joinpath('previous.js').write_text('/* retained public fixture */\n')
    try:
        command(['skopeo', 'copy', 'oci-archive:' + str(archive), 'docker-daemon:' + tag])
        # A real mount masks html/assets, but current/assets must keep serving
        # the image's exact bytes, and the older retained asset remains usable.
        container = command([
            'docker', 'run', '-d', '--read-only', '--tmpfs', '/tmp:rw,mode=1777',
            '-p', '127.0.0.1::8080',
            '--add-host', 'cma-agent-manager:127.0.0.1',
            '--add-host', 'cma-event-gateway:127.0.0.1',
            '--mount', f'type=bind,source={retained},target=/usr/share/nginx/html/assets,readonly', tag,
        ]).decode().strip()
        address = command(['docker', 'port', container, '8080/tcp']).decode().strip()
        for _ in range(50):
            try:
                with urllib.request.urlopen('http://' + address + '/build-info.json', timeout=2) as reply:
                    identity = json.load(reply)
                break
            except OSError:
                time.sleep(.2)
        else:
            raise ValueError('web-startup-failed')
        if identity['revision'] != expected_sha or identity['local'] is not False:
            raise ValueError('web-identity-differs')
        for name, digest in assets.items():
            with urllib.request.urlopen('http://' + address + '/assets/' + name, timeout=5) as reply:
                if hashlib.sha256(reply.read()).hexdigest() != digest:
                    raise ValueError('served-asset-differs')
        with urllib.request.urlopen('http://' + address + '/assets/previous.js', timeout=5) as reply:
            if reply.read() != retained.joinpath('previous.js').read_bytes():
                raise ValueError('retained-asset-differs')
        return {'current_assets_verified': len(assets), 'retained_asset_verified': True,
                'compiled_identity_verified': True}
    finally:
        if container:
            command(['docker', 'rm', '-f', container])
        command(['docker', 'image', 'rm', '-f', tag])


def measure(source, recipe, directory, revision, variant):
    directory.mkdir()
    archive = directory / 'image.oci.tar'
    start = time.monotonic()
    result = subprocess.run([
        'docker', 'buildx', 'build', '--progress=rawjson', '--platform', 'linux/amd64',
        '--provenance=false', '--sbom=false', '--file', str(recipe), '--target', 'runtime',
        '--build-arg', 'CHRONO_BUILD=git-' + revision[:12],
        '--build-arg', 'CHRONO_BUILD_SHA=' + revision,
        '--build-arg', 'OS_SECURITY_REFRESH=' + revision,
        '--output', 'type=oci,dest=' + str(archive), str(source),
    ], capture_output=True, timeout=900, check=False)
    seconds = {'build_export': round(time.monotonic() - start, 3)}
    if result.returncode:
        raise ValueError('web-build-failed')
    progress = summarize(result.stderr)
    start = time.monotonic()
    scanned = scan_archive(archive, directory / 'scan', blocking_rules='credential', component='web')
    seconds['scan'] = round(time.monotonic() - start, 3)
    report = json.loads((directory / 'scan/REPORT.json').read_bytes())
    inventory, assets = asset_inventory(archive)
    if variant == 'candidate' and inventory['hardlinks'] != inventory['asset_files_per_path']:
        raise ValueError('candidate-does-not-store-each-asset-once')
    # Same digest-preserving archive transport as publication, using a local
    # directory sink. This is an isolated byte-copy cost, not a GHCR push timing.
    start = time.monotonic()
    destination = directory / 'transport'
    command(['skopeo', 'copy', '--preserve-digests', 'oci-archive:' + str(archive), 'dir:' + str(destination)])
    manifest = command(['skopeo', 'inspect', '--raw', 'oci-archive:' + str(archive)])
    if destination.joinpath('manifest.json').read_bytes() != manifest or checksum(archive) != scanned:
        raise ValueError('transport-changed-scanned-bytes')
    seconds['local_transport_verify'] = round(time.monotonic() - start, 3)
    seconds['measured_total'] = round(sum(seconds.values()), 3)
    qualified = smoke(archive, directory, assets, revision)
    return {'seconds': seconds, 'build_progress': progress, 'archive_bytes': archive.stat().st_size,
            'archive_sha256': scanned, 'scan_counts': report['counts'], 'scan_complete': report['complete'],
            'inventory': inventory, 'smoke': qualified}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--variant', required=True, choices=['baseline', 'candidate'])
    parser.add_argument('--baseline', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    if not re.fullmatch('[0-9a-f]{40}', args.baseline):
        raise ValueError('invalid-baseline')
    args.output.mkdir(parents=True, exist_ok=True)
    revision = command(['git', 'rev-parse', 'HEAD'], cwd=ROOT).decode().strip()
    summary = {'variant': args.variant, 'source': revision, 'baseline': args.baseline,
               'platform': 'linux/amd64', 'registry_publication': False, 'runs': []}
    try:
        with tempfile.TemporaryDirectory(prefix='web-release-proof-') as temp:
            work = Path(temp)
            source = work / 'source'
            source.mkdir()
            # Only tracked public source; no workstation dependencies or secrets.
            data = command(['git', 'archive', 'HEAD'], cwd=ROOT)
            import io
            with tarfile.open(fileobj=io.BytesIO(data)) as tree:
                tree.extractall(source, filter='data')
            recipe = work / 'Dockerfile'
            recipe.write_bytes(command(['git', 'show', args.baseline + ':web/Dockerfile'], cwd=ROOT)
                               if args.variant == 'baseline' else (source / 'web/Dockerfile').read_bytes())
            summary['recipe_sha256'] = checksum(recipe)
            for name in ('cold', 'warm'):
                if name == 'warm':
                    # Same lockfile, changed application input and release identity.
                    (source / 'web/public/release-proof.txt').write_text('public application edit\n')
                    revision = hashlib.sha1((revision + '-warm').encode()).hexdigest()
                row = measure(source, recipe, work / name, revision, args.variant)
                summary['runs'].append(dict(row, name=name))
            summary['status'] = 'PASS'
    except Exception:
        summary['status'] = 'FAIL'
        raise
    finally:
        (args.output / 'RESULT.json').write_text(json.dumps(summary, indent=2) + '\n')
        print(json.dumps({'variant': args.variant, 'status': summary['status'],
                          'seconds': [row['seconds'] for row in summary['runs']]}))


if __name__ == '__main__':
    main()
