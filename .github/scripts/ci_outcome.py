"""Explain every public CI failure code without disclosing what the job read.

A failure line must tell its owner what failed, where, the likely cause and the
next step. Codes come from fixed vocabularies catalogued in ci_outcomes.json.
Every other field is checked against an enum or a pattern and prints as "?"
otherwise, so exception text, responses, paths and request data never reach
the log. This leaf module imports no repository module.

The machine lines live in this module, not in the catalogue, and the catalogue
is read only when a failure is explained. A damaged catalogue therefore never
breaks an importer, changes a parsed line or alters an exit status: reporting
falls back to the plain machine line, and the catalogue tests fail CI instead.
"""
import functools
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
COMPONENTS = frozenset({"am", "eg", "runtime", "runtime-omarchy", "web"})
TOKEN = re.compile(r"[A-Za-z][A-Za-z0-9_-]{0,47}")
SUBJECT = re.compile(r"[a-z][a-z0-9-]{0,47}")
SITE = re.compile(r"(?:[A-Za-z0-9_-]{1,64}/){0,2}[A-Za-z0-9_-]{1,64}\.py:[1-9][0-9]{0,5}")
SHA = re.compile(r"[0-9a-f]{7,40}")
HEADER = "| Failed step | Reason | Where | Likely cause | Next step |"
RULE = "|---|---|---|---|---|"
# Fields a line template does not already carry are appended in this order.
APPENDED = (("component", "component"), ("site", "at"), ("pr", "pr"), ("sha", "sha"), ("attempts", "attempts"))
# The existing machine line of each family; scripts and the controller parse these.
LINES = {
    "step": "FAIL {subject} reason={code}",
    "publication": "FAIL publication stage={stage} reason={code}",
    "finalize": "FAIL finalize stage={stage} reason={code}",
    "allocation": "FAIL allocation {code}",
    "admission": "FAIL component={component} reason={code}",
    "deploy-precondition": "FAIL precondition={code}",
    "deploy-private": "FAIL precondition=private-{code}",
    "deploy-verification": "FAIL verification={code} recovery={recovery}",
    "deploy-release": "FAIL release={code} recovery=verified",
    "deploy-component": "FAIL component={component} reason={code}",
    "deploy-clock": "FAIL: deployment not admitted; invalid or expired admission time",
    "component-result": "FAIL component-result reason={code}",
    "release-outcome": "FAIL release-outcome reason={code}",
    "component-report": "FAIL component={component} reason={code}",
    "cache-withheld": "WITHHELD build-cache reason={code}",
}
DEFAULT_LINE = "FAIL {subject} reason={code}"
UNAVAILABLE = {"title": "Explanation unavailable", "cause": "unknown",
               "meaning": "The failure catalogue could not be read, so this code has no explanation here.",
               "next": "Read the step's source at the reported check; the repository checks will name the catalogue defect."}


def check_catalogue(value):
    """Refuse a catalogue whose shape the renderer could not trust."""
    def complete(item):
        return (isinstance(item, dict) and set(item) == {"title", "meaning", "cause", "next"}
                and item["cause"] in CAUSES and all(isinstance(item[key], str) for key in ("title", "meaning", "next")))
    families = value.get("families") if isinstance(value, dict) else None
    if not (isinstance(families, dict) and complete(value.get("uncatalogued"))
            and isinstance(value.get("causes"), dict) and set(value["causes"]) == set(CAUSES)):
        raise ValueError("failure catalogue shape is invalid")
    for spec in families.values():
        table = families.get(spec.get("codes_from"), {}).get("codes") if isinstance(spec, dict) else None
        table = spec.get("codes") if isinstance(spec, dict) and "codes" in spec else table
        if not (isinstance(table, dict) and all(complete(item) for item in table.values())):
            raise ValueError("failure catalogue family is invalid")
    return value


@functools.lru_cache(maxsize=1)
def catalogue():
    """Read the catalogue on first use; raises when it is missing or malformed."""
    with (ROOT / "ci_outcomes.json").open(encoding="utf-8") as stream:
        return check_catalogue(json.load(stream))


def codes(family):
    """Return a family's code entries, following a shared vocabulary; {} if unavailable."""
    try:
        families = catalogue()["families"]
    except Exception:  # noqa: BLE001 - a damaged catalogue explains nothing but breaks nothing.
        return {}
    spec = families.get(family) if isinstance(family, str) else None
    if not isinstance(spec, dict):
        return {}
    return families[spec["codes_from"]]["codes"] if "codes_from" in spec else spec["codes"]


def entry(family, code):
    """Return the complete entry for a code, the drift entry, or a fixed fallback."""
    try:
        drift = catalogue()["uncatalogued"]
    except Exception:  # noqa: BLE001 - a damaged catalogue explains nothing but breaks nothing.
        return UNAVAILABLE
    found = codes(family).get(code) if isinstance(code, str) else None
    return found if found is not None else drift


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
        spec = catalogue()["families"].get(family) if isinstance(family, str) else None
        stages = codes(spec.get("stages")) if isinstance(spec, dict) else {}
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


def plain_field(name, value):
    """Validate a field without the catalogue: enums where they live here, else a bare token."""
    if name in ("code", "stage", "recovery"):
        return value if isinstance(value, str) and TOKEN.fullmatch(value) else UNKNOWN
    return field(name, value)


def machine_line(family, values):
    template = LINES.get(family, DEFAULT_LINE)
    line = template.format(**{name: values.get(name, UNKNOWN) for name in PLACEHOLDERS})
    for name, label in APPENDED:
        if name in values and "{" + name + "}" not in template:
            line += f" {label}={values[name]}"
    return line


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
    the innermost traceback frame that belongs to these public scripts. The
    frame that caught the exception names only its call, not the failure, so
    it counts only when the exception was raised there.
    """
    if hasattr(error, "site"):
        # A refusal raised outside these scripts has no public site; its
        # traceback would only name the shared helper that raised it.
        recorded = error.site
        return recorded if isinstance(recorded, str) and SITE.fullmatch(recorded) else None
    site, trace = None, getattr(error, "__traceback__", None)
    if trace is not None and trace.tb_next is not None:
        trace = trace.tb_next
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
    """Return (machine line, annotation, summary row) built from validated fields only.

    Raises when the catalogue is unavailable; report() then prints the plain line.
    """
    known = catalogue()
    spec = known["families"].get(family) if isinstance(family, str) else None
    spec, family = (spec, family) if isinstance(spec, dict) and family in LINES else ({}, UNKNOWN)
    token = code if isinstance(code, str) and code in codes(family) else UNKNOWN
    found = entry(family, token)
    values = {"code": token, "subject": field("subject", subject)}
    given = {"stage": stage, "component": component, "recovery": recovery, "site": site,
             "pr": pr, "sha": sha, "attempts": attempts}
    values.update({name: field(name, value, family) for name, value in given.items() if value is not None})

    line = machine_line(family, values)
    stage_entry = codes(spec.get("stages")).get(values.get("stage"))
    cause = found["cause"]
    if cause == "unknown" and stage_entry is not None:
        cause = stage_entry["cause"]
    line += " cause=" + cause
    next_step = found["next"]

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
    details.append(f"Likely cause: {cause}. {known['causes'][cause]}")
    details.append("Next step: " + next_step)
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
        values["subject"], f"{token}: {found['title']}", "; ".join(where) or "-", cause, next_step)) + " |"
    return line, annotation, row


def plain(family, code, *, subject, stage=None, site=None, component=None, pr=None, sha=None,
          attempts=None, recovery=None):
    """The machine line alone, validated without the catalogue."""
    values = {"code": plain_field("code", code), "subject": field("subject", subject)}
    given = {"stage": stage, "component": component, "recovery": recovery, "site": site,
             "pr": pr, "sha": sha, "attempts": attempts}
    values.update({name: plain_field(name, value) for name, value in given.items() if value is not None})
    return machine_line(family if family in LINES else None, values)


def summarize(path, row):
    """Append a row, continuing this module's table if the file ends with it.

    Each table ends in a blank line, so later text never renders as a row.
    """
    target = Path(path)
    try:
        text = target.read_text(encoding="utf-8")
    except FileNotFoundError:
        text = ""
    lines = text.split("\n")
    if text.endswith("\n\n"):
        table = []
        for line in reversed(lines[:-2]):
            if not line.startswith("|"):
                break
            table.insert(0, line)
        if table[:2] == [HEADER, RULE]:
            with target.open("r+b") as stream:
                stream.seek(len(text.encode("utf-8")) - 1)
                stream.write((row + "\n\n").encode("utf-8"))
            return
    prefix = "" if not text else ("\n" if text.endswith("\n") else "\n\n")
    with target.open("a", encoding="utf-8") as stream:
        stream.write(prefix + HEADER + "\n" + RULE + "\n" + row + "\n\n")


def report(family, code, *, subject, stage=None, site=None, component=None, pr=None, sha=None,
           attempts=None, recovery=None, stream=None, env=None):
    """Print one machine line, one annotation and one summary row; never raise.

    Without a readable catalogue only the plain machine line is printed. Returns
    the machine line so callers can keep recording it where they did.
    """
    stream = sys.stdout if stream is None else stream
    env = os.environ if env is None else env
    fields = dict(subject=subject, stage=stage, site=site, component=component, pr=pr, sha=sha,
                  attempts=attempts, recovery=recovery)
    try:
        line, annotation, row = render(family, code, **fields)
    except Exception:  # noqa: BLE001 - reporting must never replace the caller's line or exit status.
        try:
            line = plain(family, code, **fields)
        except Exception:  # noqa: BLE001 - the last resort still names the failed step.
            line = "FAIL " + (subject if isinstance(subject, str) and SUBJECT.fullmatch(subject) else UNKNOWN)
        annotation = row = None
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
