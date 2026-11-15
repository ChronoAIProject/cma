"""Download the public localhost.direct fixture; never use a Secret or skip TLS."""
import hashlib
import io
from pathlib import Path
import re
import subprocess
import urllib.request
import zipfile

ARCHIVE_SHA256 = '775a3517f5c7c1b62165d72dff98e092b432502d454a0dcd687b2463c2832ad8'
FILES = {
    'localhost.direct.OPEN.crt': '81367d4b01e92b9172a93d1438b2f412fc7ccdfa59c9624e0e742d6b2effe13a',
    'localhost.direct.OPEN.key': 'f2a383d5e1b7f4d10f1ce4019a4509335e30ba4ba1bccb6deab7d34700b360ff',
}


def download(url):
    with urllib.request.urlopen(url, timeout=30) as response:
        data = response.read(4 * 1024 * 1024 + 1)
    if len(data) > 4 * 1024 * 1024:
        raise ValueError('public_tls_bundle_size')
    return data


def prepare(directory):
    directory = Path(directory)
    directory.mkdir(mode=0o700)
    archive = download('https://aka.re/localhost')
    if hashlib.sha256(archive).hexdigest() != ARCHIVE_SHA256:
        raise ValueError('public_tls_bundle_rotated_requires_review')
    readme = download('https://raw.githubusercontent.com/Upinel/localhost.direct/master/README.md').decode()
    # Both passwords are publicly documented. Only the exact pinned public-CA
    # members can be selected, so the self-signed bundle cannot be substituted.
    passwords = re.findall(r'Password:\s*\*\*([^*]+)\*\*', readme)
    with zipfile.ZipFile(io.BytesIO(archive)) as bundle:
        for name, expected in FILES.items():
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
    for hostname in ['provider.localhost.direct', 'api.localhost.direct']:
        subprocess.run(['openssl', 'x509', '-in', str(certificate), '-checkhost', hostname, '-noout'],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return directory
