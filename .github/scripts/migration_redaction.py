"""Redact generated fixture credentials before retaining command evidence."""
import json
import re
from urllib.parse import quote, unquote, urlsplit


SENSITIVE = re.compile(r'(?:password|secret|token|bearer|encryption_key|keyfile|control[-_]key)$', re.I)


class EvidenceRedactor:
    def __init__(self):
        self.values = set()

    def remember(self, value):
        if isinstance(value, str) and value:
            self.values.update((value, json.dumps(value)[1:-1], quote(value, safe='')))

    def collect(self, value):
        """Inspect only locally generated manifests, never cluster Secrets."""
        if isinstance(value, list):
            for item in value:
                self.collect(item)
        elif isinstance(value, dict):
            name = value.get('name')
            if isinstance(name, str) and SENSITIVE.search(name):
                self.remember(value.get('value'))
            for key, item in value.items():
                if SENSITIVE.search(key):
                    self.remember(item)
                if key.upper().endswith('_URI') and isinstance(item, str):
                    password = urlsplit(item).password
                    if password:
                        self.remember(unquote(password))
                if key == 'identity.json' and isinstance(item, str):
                    self.collect(json.loads(item))
                else:
                    self.collect(item)

    def text(self, value):
        if isinstance(value, bytes):
            value = value.decode('utf-8', errors='replace')
        for credential in sorted(self.values, key=len, reverse=True):
            value = value.replace(credential, '[REDACTED]')
        return value
