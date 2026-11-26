"""Download the public localhost.direct fixture; never use a Secret or skip TLS."""
import hashlib
import io
from pathlib import Path
import re
import subprocess
import urllib.request
import zipfile

ARCHIVE_SHA256 = '775a3517f5c7c1b62165d72dff98e092b432502d454a0dcd687b2463c2832ad8'
ARCHIVE_URL = 'https://get.localhost.direct/localhost.direct.OP.zip'
def pinned_files():
    # Conventional checksum records keep public digests distinct from key material.
    rows = [line.split('  ', 1) for line in Path(__file__).with_name('localhost-direct.sha256').read_text().splitlines()]
    if (len(rows) != 2 or any(len(row) != 2 or not re.fullmatch('[0-9a-f]{64}', row[0]) for row in rows)
            or {row[1] for row in rows} != {'localhost.direct.OPEN.crt', 'localhost.direct.OPEN.key'}):
        raise ValueError('public_tls_pin_format')
    return {name: digest for digest, name in rows}


def download(url):
    request = urllib.request.Request(url, headers={
        'User-Agent': 'cma-two-am-ci/1.0 (+https://github.com/ChronoAIProject/cma)'})
    with urllib.request.urlopen(request, timeout=30) as response:
        data = response.read(4 * 1024 * 1024 + 1)
    if len(data) > 4 * 1024 * 1024:
        raise ValueError('public_tls_bundle_size')
    return data


def prepare(directory):
    directory = Path(directory)
    directory.mkdir(mode=0o700)
    # Official destination of the public download link. The shortener rejects
    # urllib's default agent with 403; the archive itself remains hash-pinned.
    archive = download(ARCHIVE_URL)
    if hashlib.sha256(archive).hexdigest() != ARCHIVE_SHA256:
        raise ValueError('public_tls_bundle_rotated_requires_review')
    readme = download('https://raw.githubusercontent.com/Upinel/localhost.direct/master/README.md').decode()
    # Both passwords are publicly documented. Only the exact pinned public-CA
    # members can be selected, so the self-signed bundle cannot be substituted.
    passwords = re.findall(r'Password:\s*\*\*([^*]+)\*\*', readme)
    with zipfile.ZipFile(io.BytesIO(archive)) as bundle:
        for name, expected in pinned_files().items():
            data = None
            for password in passwords:
                try:
                    data = bundle.read(name, pwd=password.encode())
                    break
                except RuntimeError:
                    continue
            if data is None or hashlib.sha256(data).hexdigest() != expected:
                raise ValueError('public_tls_fixture_hash')
            (directory / name).write_bytes(data)
            (directory / name).chmod(0o600)
    certificate = directory / 'localhost.direct.OPEN.crt'
    key = directory / 'localhost.direct.OPEN.key'
    subprocess.run(['openssl', 'x509', '-in', str(certificate), '-checkend', '86400', '-noout'],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    public_cert = subprocess.check_output(['openssl', 'x509', '-in', str(certificate), '-pubkey', '-noout'])
    public_key = subprocess.check_output(['openssl', 'pkey', '-in', str(key), '-pubout'], stderr=subprocess.DEVNULL)
    if public_cert != public_key:
        raise ValueError('public_tls_key_mismatch')
    for hostname in ['provider.localhost.direct', 'api.localhost.direct', 'storage.localhost.direct']:
        subprocess.run(['openssl', 'x509', '-in', str(certificate), '-checkhost', hostname, '-noout'],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return directory
