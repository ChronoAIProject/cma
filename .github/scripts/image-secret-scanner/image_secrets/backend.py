"""Gitleaks adapter. Raw reports are private, transient, and never published."""

import os
import re
import subprocess

from .common import ScanError, digest_bytes, digest_file
from .detector_report import report_rows


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
        self.cache = None

    def scan(self, stage, locations, findings):
        return self.detect(stage, locations, findings)

    @staticmethod
    def report(findings, rule, location, candidate, line, staged_digest):
        # ELF detector views have synthetic line numbers. Bind reports to the
        # original file for both fresh detections and cache replays.
        if "_file_sha256" in location:
            location = location.copy()
            staged_digest = location.pop("_file_sha256")
            line = None
        findings.add(rule, location, candidate, line, file_digest=staged_digest)

    def scan_layer(self, stage, locations, findings, diff_id):
        if self.cache is None:
            return self.scan(stage, locations, findings)
        digests = {physical: digest_file(physical) for physical in locations}
        coverage = [{"name": p.name, "size": p.stat().st_size, "sha256": digests[p]} for p in locations]
        rows = self.cache.lookup(diff_id, coverage)
        if rows is not None:
            by_name = {p.name: p for p in locations}
            for row in rows:
                physical = by_name[row["input"]]
                self.report(findings, row["rule"], locations[physical], row["candidate_sha256"],
                            row["line"], digests[physical])
            return
        observed = []
        if self.detect(stage, locations, findings, observed=observed, digests=digests):
            self.cache.store(diff_id, coverage, observed)

    def detect(self, stage, locations, findings, *, observed=None, digests=None):
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
        counts = []
        for line in messages:
            if not line.strip():
                continue
            match = re.fullmatch(r".*\bWRN leaks found: ([0-9]{1,12})\s*", line)
            if match is None:
                raise ScanError("gitleaks-incomplete")
            counts.append(int(match.group(1)))
        if result.returncode not in (0, 1) or not raw.is_file() or len(counts) > 1:
            raise ScanError("gitleaks-incomplete")
        detector_count = counts[0] if counts else 0
        if bool(detector_count) != (result.returncode == 1):
            raise ScanError("gitleaks-result-inconsistent")
        index = self.index(stage, locations)
        digests = {} if digests is None else digests
        cacheable = True
        mapped = []
        for row in report_rows(raw):
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
            location, physical = self.resolve_index(name, index)
            line = row.get("StartLine")
            if not isinstance(line, int) or line < 0:
                raise ScanError("invalid-gitleaks-line")
            if physical not in digests:
                digests[physical] = digest_file(physical)
            mapped.append((physical.name, "gitleaks." + rule, digest_bytes(secret.encode()), line, location, physical))
        if len(mapped) != detector_count:
            raise ScanError("gitleaks-result-inconsistent")
        # Gitleaks workers may return rows in different orders. Canonical mapping
        # makes cold and reused evidence identical without dropping occurrences.
        mapped.sort(key=lambda value: value[:4])
        for _, rule, candidate, line, location, physical in mapped:
            if observed is not None:
                if location != locations[physical]:
                    cacheable = False
                else:
                    observed.append({"input": physical.name, "rule": rule,
                                     "candidate_sha256": candidate, "line": line})
            self.report(findings, rule, location, candidate, line, digests[physical])
        raw.unlink()
        error_log.unlink()
        return cacheable

    @staticmethod
    def locate(name, stage, locations):
        return Gitleaks.resolve(name, stage, locations)[0]

    @staticmethod
    def resolve(name, stage, locations):
        return Gitleaks.resolve_index(name, Gitleaks.index(stage, locations))

    @staticmethod
    def index(stage, locations):
        result = {}
        for physical, location in locations.items():
            for name in (str(physical), physical.name, str(physical.relative_to(stage))):
                if name in result and result[name][1] != physical:
                    raise ScanError("ambiguous-gitleaks-path")
                result[name] = (location, physical)
        return result

    @staticmethod
    def resolve_index(name, index):
        if name in index:
            return index[name]
        # Synthetic names contain no '!'; preserve nested origin mapping.
        prefix, separator, suffix = name.partition("!")
        if separator and prefix in index:
            location, physical = index[prefix]
            return dict(location, path=location["path"] + "!" + suffix), physical
        raise ScanError("unmapped-gitleaks-path")
