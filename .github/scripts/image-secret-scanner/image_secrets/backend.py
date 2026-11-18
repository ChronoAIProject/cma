"""Gitleaks adapter. Raw reports are private, transient, and never published."""

import os
import re
import subprocess

from .common import ScanError, digest_bytes, digest_file, json_bytes


class Gitleaks:
    def __init__(self, executable, rules, workspace, timeout):
        self.executable, self.rules = executable, rules
        self.workspace, self.timeout = workspace, timeout
        self.ignore = workspace / "empty.gitleaksignore"
        self.ignore.write_text("")
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("GITLEAKS_")}
        result = subprocess.run(
            [executable, "version"], check=False, capture_output=True, env=self.env, timeout=timeout
        )
        version = result.stdout.decode("ascii", "replace").strip()
        if result.returncode or not re.fullmatch(r"8\.(?:2[5-9]|[3-9][0-9])\.[0-9]+", version):
            raise ScanError("gitleaks-8-25-or-newer-required")
        self.version = version

    def scan(self, stage, locations, findings):
        raw = self.workspace / "gitleaks-private.json"
        error_log = self.workspace / "gitleaks-private.stderr"
        raw.unlink(missing_ok=True)
        argv = [
            self.executable,
            "dir",
            str(stage),
            "--config",
            str(self.rules),
            "--gitleaks-ignore-path",
            str(self.ignore),
            "--ignore-gitleaks-allow",
            "--no-banner",
            "--no-color",
            "--log-level",
            "warn",
            "--redact=0",
            "--max-target-megabytes",
            "0",
            "--max-archive-depth",
            "0",
            "--max-decode-depth",
            "5",
            "--timeout",
            str(self.timeout),
            "--report-format",
            "json",
            "--report-path",
            str(raw),
        ]
        # Values are needed privately for exact-value exceptions; stdout/stderr never reach a terminal.
        with error_log.open("wb") as errors:
            result = subprocess.run(
                argv,
                check=False,
                cwd=self.workspace,
                env=self.env,
                stdout=subprocess.DEVNULL,
                stderr=errors,
                timeout=self.timeout + 5,
            )
        # A depth-limit, unreadable archive, or other skipped input invalidates CLEAN.
        # The only expected warning is Gitleaks' final finding count; never publish log text.
        messages = error_log.read_text(errors="replace").splitlines()
        unexpected_warning = any(
            not re.fullmatch(r".*\bWRN leaks found: \d+\s*", line) for line in messages if line.strip()
        )
        if result.returncode not in (0, 1) or not raw.is_file() or unexpected_warning:
            raise ScanError("gitleaks-incomplete")
        if raw.stat().st_size > 64 * 1024 * 1024:
            raise ScanError("gitleaks-report-too-large")
        rows = json_bytes(raw.read_bytes())
        if not isinstance(rows, list) or (bool(rows) != (result.returncode == 1)):
            raise ScanError("gitleaks-result-inconsistent")
        lookup = self.index_locations(stage, locations)
        file_digests = {}
        for row in rows:
            if not isinstance(row, dict):
                raise ScanError("invalid-gitleaks-finding")
            name, rule, secret = row.get("File"), row.get("RuleID"), row.get("Secret")
            if (
                not isinstance(name, str)
                or not isinstance(rule, str)
                or not isinstance(secret, str)
                or not re.fullmatch(r"[a-zA-Z0-9_.-]+", rule)
            ):
                raise ScanError("invalid-gitleaks-finding")
            location, physical = self.resolve(name, stage, locations, lookup=lookup)
            line = row.get("StartLine")
            if not isinstance(line, int) or line < 0:
                raise ScanError("invalid-gitleaks-line")
            # The private stage is complete and immutable during detection.
            # Rehashing large binaries for every entropy finding is unbounded
            # repeated work; each finding still binds its complete staged file.
            if physical not in file_digests:
                file_digests[physical] = digest_file(physical)
            findings.add(
                "gitleaks." + rule,
                location,
                digest_bytes(secret.encode()),
                line,
                file_digest=file_digests[physical],
            )
        raw.unlink()
        error_log.unlink()

    @staticmethod
    def locate(name, stage, locations):
        return Gitleaks.resolve(name, stage, locations)[0]

    @staticmethod
    def index_locations(stage, locations):
        lookup = {}
        for physical, location in locations.items():
            for prefix in (str(physical), physical.name, str(physical.relative_to(stage))):
                if prefix in lookup and lookup[prefix][1] != physical:
                    raise ScanError("ambiguous-gitleaks-path")
                lookup[prefix] = (location, physical)
        return lookup

    @staticmethod
    def resolve(name, stage, locations, *, lookup=None):
        lookup = Gitleaks.index_locations(stage, locations) if lookup is None else lookup
        if name in lookup:
            return lookup[name]
        # Only exact registered aliases may introduce a tool-generated suffix.
        # Work backwards so an exclamation mark in the private parent directory
        # cannot shorten the actual file identity. Never match a string prefix.
        boundary = name.rfind("!")
        while boundary >= 0:
            matched = lookup.get(name[:boundary])
            if matched is not None:
                location, physical = matched
                return dict(location, path=location["path"] + name[boundary:]), physical
            boundary = name.rfind("!", 0, boundary)
        raise ScanError("unmapped-gitleaks-path")
