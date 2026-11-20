"""Explain every public CI failure code without disclosing what the job read.

A failure line must tell its owner what failed, where, the likely cause and the
next step. Codes come from fixed vocabularies catalogued in ci_outcomes.json.
Every other field is checked against an enum or a pattern and prints as "?"
otherwise, so exception text, responses, paths and request data never reach
the log. This leaf module imports no repository module.
"""
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
FILE_PREFIX = ".github/scripts/"
UNKNOWN = "?"
CAUSES = ("change", "transient", "operator", "unknown")
LEVELS = ("error", "warning")
PLACEHOLDERS = ("code", "stage", "component", "recovery", "subject")
SUBJECT = re.compile(r"[a-z][a-z0-9-]{0,47}")
SITE = re.compile(r"(?:[A-Za-z0-9_-]{1,64}/){0,2}[A-Za-z0-9_-]{1,64}\.py:[1-9][0-9]{0,5}")
SHA = re.compile(r"[0-9a-f]{7,40}")
HEADER = "| Failed step | Reason | Where | Likely cause | Next step |"
RULE = "|---|---|---|---|---|"
# Fields a line template does not already carry are appended in this order.
APPENDED = (("component", "component"), ("site", "at"), ("pr", "pr"), ("sha", "sha"), ("attempts", "attempts"))

with (ROOT / "ci_outcomes.json").open(encoding="utf-8") as _stream:
    CATALOGUE = json.load(_stream)
FAMILIES = CATALOGUE["families"]
COMPONENTS = frozenset(CATALOGUE["components"])


def codes(family):
    """Return a family's code entries, following a shared vocabulary."""
    spec = FAMILIES.get(family) if isinstance(family, str) else None
    if spec is None:
        return {}
    return FAMILIES[spec["codes_from"]]["codes"] if "codes_from" in spec else spec["codes"]


def entry(family, code):
    """Return the complete entry for a code, or the catalogue's drift entry."""
    found = codes(family).get(code) if isinstance(code, str) else None
    return found if found is not None else CATALOGUE["uncatalogued"]


def field(name, value, family=None):
    """Return a printable field value: an allowed token or "?"."""
    if name == "subject":
        return value if isinstance(value, str) and SUBJECT.fullmatch(value) else UNKNOWN
    if name == "component":
        values = [value] if isinstance(value, str) else value
        if isinstance(values, (list, tuple, set, frozenset)) and values \
                and all(isinstance(item, str) and item in COMPONENTS for item in values):
            return ",".join(sorted(set(values)))
        return UNKNOWN
    if name == "stage":
        stages = codes(FAMILIES.get(family, {}).get("stages")) if isinstance(family, str) else {}
        return value if isinstance(value, str) and value in stages else UNKNOWN
    if name == "recovery":
        return value if isinstance(value, str) and value in codes("deploy-verification") else UNKNOWN
    if name == "site":
        return value if isinstance(value, str) and SITE.fullmatch(value) else UNKNOWN
    if name == "sha":
        return value if isinstance(value, str) and SHA.fullmatch(value) else UNKNOWN
    if name in ("pr", "attempts"):
        limit = 10_000_000 if name == "pr" else 1000
        return str(value) if type(value) is int and 0 < value < limit else UNKNOWN
    return UNKNOWN


def source_site(filename, line):
    """Name a public script location as path:line relative to this directory."""
    try:
        path = Path(filename).resolve().relative_to(ROOT)
    except (TypeError, ValueError, OSError):
        return None
    site = f"{path.as_posix()}:{line}"
    return site if SITE.fullmatch(site) else None


def error_site(error):
    """Return where a failure was raised, never what it said.

    Refusals record their raising line when constructed. Other exceptions use
    the innermost traceback frame that belongs to these public scripts.
    """
    recorded = getattr(error, "site", None)
    if isinstance(recorded, str) and SITE.fullmatch(recorded):
        return recorded
    site, trace = None, getattr(error, "__traceback__", None)
    while trace is not None:
        site = source_site(trace.tb_frame.f_code.co_filename, trace.tb_lineno) or site
        trace = trace.tb_next
    return site


def escape_data(text):
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def escape_property(text):
    return escape_data(text).replace(":", "%3A").replace(",", "%2C")


def cell(text):
    return " ".join(text.replace("\\", "\\\\").replace("|", "\\|").split())


def render(family, code, *, subject, stage=None, site=None, component=None, pr=None, sha=None,
           attempts=None, recovery=None):
    """Return (machine line, annotation, summary row) built from validated fields only."""
    spec = FAMILIES.get(family) if isinstance(family, str) else None
    spec, family = (spec, family) if isinstance(spec, dict) else ({}, UNKNOWN)
    token = code if isinstance(code, str) and code in codes(family) else UNKNOWN
    found = entry(family, token)
    values = {"code": token, "subject": field("subject", subject)}
    given = {"stage": stage, "component": component, "recovery": recovery, "site": site,
             "pr": pr, "sha": sha, "attempts": attempts}
    values.update({name: field(name, value, family) for name, value in given.items() if value is not None})

    template = spec.get("line", "FAIL {subject} reason={code}")
    line = template.format(**{name: values.get(name, UNKNOWN) for name in PLACEHOLDERS})
    for name, label in APPENDED:
        if name in values and "{" + name + "}" not in template:
            line += f" {label}={values[name]}"
    stage_entry = codes(spec.get("stages")).get(values.get("stage"))
    cause = found["cause"]
    if cause == "unknown" and stage_entry is not None:
        cause = stage_entry["cause"]
    line += " cause=" + cause

    details, where = [found["meaning"]], []
    if stage_entry is not None:
        details.append(f"Stage {values['stage']}: {stage_entry['meaning']}")
        where.append("stage " + values["stage"])
    if "component" in values:
        details.append("Component: " + values["component"])
        where.append(values["component"])
    if "site" in values:
        details.append("Check: " + (FILE_PREFIX + values["site"] if values["site"] != UNKNOWN else UNKNOWN))
        where.append(values["site"])
    if "recovery" in values:
        recovered = entry("deploy-verification", values["recovery"])
        details.append(f"Recovery check: {values['recovery']}. {recovered['meaning']}")
    if "attempts" in values:
        details.append("Attempts: " + values["attempts"])
    if spec.get("effect"):
        details.append("What changed: " + spec["effect"])
    details.append(f"Likely cause: {cause}. {CATALOGUE['causes'][cause]}")
    details.append("Next step: " + found["next"])
    if stage_entry is not None:
        details.append("At this stage: " + stage_entry["next"])
    details.append(f"Reference: {family}/{token} in {FILE_PREFIX}ci_outcomes.json")

    properties = []
    if values.get("site", UNKNOWN) != UNKNOWN:
        path, number = values["site"].rsplit(":", 1)
        properties += ["file=" + escape_property(FILE_PREFIX + path), "line=" + number]
    heading = f"{values['subject']}: " + (spec["title"] + ": " if spec.get("title") else "") + found["title"]
    properties.append("title=" + escape_property(heading))
    level = spec.get("level") if spec.get("level") in LEVELS else "error"
    annotation = f"::{level} {','.join(properties)}::{escape_data(chr(10).join(details))}"
    row = "| " + " | ".join(cell(text) for text in (
        values["subject"], f"{token}: {found['title']}", "; ".join(where) or "-", cause, found["next"])) + " |"
    return line, annotation, row


def summarize(path, row):
    """Append a row, starting a table unless the file already ends in one."""
    target = Path(path)
    try:
        lines = target.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        lines = []
    table = []
    for line in reversed(lines):
        if not line.startswith("|"):
            break
        table.insert(0, line)
    prefix = "" if table[:2] == [HEADER, RULE] else ("\n" if lines else "") + HEADER + "\n" + RULE + "\n"
    with target.open("a", encoding="utf-8") as stream:
        stream.write(prefix + row + "\n")


def report(family, code, *, subject, stage=None, site=None, component=None, pr=None, sha=None,
           attempts=None, recovery=None, stream=None, env=None):
    """Print one machine line, one annotation and one summary row; never raise.

    Returns the machine line so callers can keep recording it where they did.
    """
    stream = sys.stdout if stream is None else stream
    env = os.environ if env is None else env
    try:
        line, annotation, row = render(family, code, subject=subject, stage=stage, site=site,
                                       component=component, pr=pr, sha=sha, attempts=attempts,
                                       recovery=recovery)
    except Exception:  # noqa: BLE001 - reporting must never replace the caller's exit status.
        line, annotation, row = f"FAIL {field('subject', subject)}", None, None
    print(line, file=stream, flush=True)
    if annotation is not None:
        print(annotation, file=stream, flush=True)
    summary = env.get("GITHUB_STEP_SUMMARY")
    if summary and row is not None:
        try:
            summarize(summary, row)
        except (OSError, UnicodeDecodeError):
            pass  # The log already carries the line and the annotation.
    return line
