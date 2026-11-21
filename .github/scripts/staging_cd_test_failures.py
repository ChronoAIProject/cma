"""Name the failing tests of allowlisted test stages, and nothing else, from private BuildKit logs.

A Dockerfile step's output stays private: it can quote build arguments,
mounted files or fixture data. A stage listed here only runs this public
repository's test suites on public inputs, and PR CI already prints its whole
output; that is the boundary. Within it, only fields of each reporter's own
failure report leave this module (a title, a test id, a file basename): node's
last spec summary on stdout, unittest's final error list on stderr, and fixed
media-self-test lines. Each field then passes a fixed charset, a length bound
and secret-shape checks, which are defense in depth rather than a boundary:
text a test prints in exactly a reporter's failure format, where that report
is read, can still be taken for a name.
"""
import base64
import binascii
import json
import re

# Each listed stage only runs tests; every other stage's output is never read.
# runtime-qualification (layer4-runtime/Dockerfile): `node --test` over the
#   codex adapter's *.test.cjs files and the two unittest scripts
#   recovery_test.py and recovery_desktop_test.py, then a fixed marker file.
# media-self-test (cmaeg/Dockerfile): `cmaeg --media-self-test` prints one
#   fixed-format line per committed fixture: name, op and outcome tokens only.
STAGES = {"runtime-qualification": ("tap", "spec", "unittest"), "media-self-test": ("media",)}
# BuildKit records a RUN step's stdout as stream 1 and its stderr as stream 2.
# node --test writes its reporter to stdout and forwards each test file's own
# stdout and stderr there; unittest writes only to stderr; the media report is
# printed with cat. Each parser reads only its own stream.
STREAM = {"tap": 1, "spec": 1, "unittest": 2, "media": 1}
# Bytes kept per vertex and stream: the start holds early TAP failures and the
# end holds spec and unittest summaries. The middle of a larger log is dropped.
HEAD = TAIL = 256 * 1024
YAML_LINES = 400
TRACE_LINES = 400
MAX_NAMES = 10
MAX_NAME = 120
STREAMS = (1, 2)

ANSI = re.compile(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)|[@-Z\\-_])")
DROPPED = re.compile(r"[^A-Za-z0-9 _.,:/()<>=+#-]")
NAME = re.compile(r"[A-Za-z0-9 _.,:/()<>=+#-]{1,%d}" % MAX_NAME)
FILE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,79}")
STEP = re.compile(r"([A-Za-z0-9][A-Za-z0-9_.-]{0,63}) [1-9][0-9]{0,3}/[1-9][0-9]{0,3}")
# A whole node test file that failed to load reports its path as the title.
SCRIPT = re.compile(r"/?(?:[A-Za-z0-9_.-]+/)+([A-Za-z0-9_.-]+\.[cm]?js)")
# Rejected whole, never trimmed: shapes of keys, tokens and credentials.
SECRET_SHAPES = (
    re.compile(r"[0-9A-Fa-f]{16,}"),
    re.compile(r"[A-Za-z0-9+=]{40,}"),
    re.compile(r"(?i)[a-z][a-z0-9+.-]*://[^\s/]*@"),
    re.compile(r"=\s*[\"']?[^\s\"',;)]{8,}"),
    re.compile(r"(?i)(?:passw(?:or)?d|secret|token|api[_-]?key|authorization|credential|cookie|private[_ -]?key)s?\s*[:=]"),
    re.compile(r"(?i)\bbearer\s+\S"),
    re.compile(r"(?:\bgh[pousr]_|github_pat_|\bglpat-|\bxox[abposr]-|\bsk-[A-Za-z0-9]|\bA[KS]IA[0-9A-Z]{8}|\bAIza|\beyJ[A-Za-z0-9_-]{4}|-----BEGIN)"),
)
MIXED_RUN = re.compile(r"[A-Za-z0-9+/=_-]{20,}")
DIGIT_RUN = re.compile(r"[A-Za-z0-9+=]{24,}")

TAP_HEADER = "TAP version 13"
TAP = re.compile(r"( *)(not ok|ok) [0-9]{1,6}(?: - (.*))?")
DIRECTIVE = re.compile(r"(?<!\\)# (?:TODO|SKIP)\b", re.IGNORECASE)
LOCATION = re.compile(r"(.+):[0-9]+:[0-9]+")
SPEC_HEADER = "✖ failing tests:"
SPEC_AT = re.compile(r"test at (.+):[0-9]+:[0-9]+")
SPEC_FAIL = re.compile(r"✖ (.+) \([0-9]+(?:\.[0-9]+)?m?s\)")
UNITTEST = re.compile(r"(?:FAIL|ERROR): ([A-Za-z_][A-Za-z0-9_]{0,127}) \(([A-Za-z_][A-Za-z0-9_.]{0,255})\)(?: .*)?")
SEPARATOR1, SEPARATOR2 = "=" * 70, "-" * 70
RAN = re.compile(r"Ran [0-9]+ tests? in [0-9]+\.[0-9]+s")
TOTALS = re.compile(r"FAILED \(((?:[a-z ]+=[0-9]+)(?:, [a-z ]+=[0-9]+)*)\)")
FRAME = re.compile(r'  File "(.+)", line [0-9]+, in ([A-Za-z_][A-Za-z0-9_]*)')
MEDIA = re.compile(r"media-self-test case=([A-Za-z0-9][A-Za-z0-9_.-]{0,63}) op=(keyframes|audio) "
                   r"outcome=([a-z_]{1,32}) frames=[0-9]+ pcm_bytes=[0-9]+ ms=[0-9]+")


class Retained:
    """At most HEAD + TAIL bytes of one vertex stream."""

    def __init__(self):
        self.head, self.tail, self.gap = bytearray(), bytearray(), False

    def add(self, chunk):
        room = HEAD - len(self.head)
        if room > 0:
            self.head += chunk[:room]
            chunk = chunk[room:]
        if len(chunk) > TAIL:
            chunk, self.gap = chunk[-TAIL:], True
        self.tail += chunk
        if len(self.tail) > TAIL:
            del self.tail[:len(self.tail) - TAIL]
            self.gap = True

    def text(self):
        head, tail = bytes(self.head), bytes(self.tail)
        if self.gap:
            # Neither partial line beside the dropped middle may form a match.
            head = head[:head.rfind(b"\n") + 1]
            cut = tail.find(b"\n")
            tail = tail[cut + 1:] if cut >= 0 else b""
        return (head + tail).decode("utf-8", errors="replace")


def secret_like(text):
    if any(shape.search(text) for shape in SECRET_SHAPES):
        return True
    if any(re.search(r"[0-9]", run) for run in DIGIT_RUN.findall(text)):
        return True
    return any(re.search(r"[0-9]", run) and re.search(r"[A-Z]", run) and re.search(r"[a-z]", run)
               for run in MIXED_RUN.findall(text))


def clean_name(text):
    """A captured title or test id, reduced to the public charset, or None."""
    text = ANSI.sub("", text)
    if secret_like(text):
        return None
    value = " ".join(DROPPED.sub("", re.sub(r"\s", " ", text)).split())
    if not value or "::" in value or secret_like(value):
        return None
    return value[:MAX_NAME].rstrip()


def clean_file(path):
    """The basename of a captured test file path, or None."""
    base = path.rsplit("/", 1)[-1]
    if not FILE.fullmatch(base) or secret_like(base):
        return None
    return base


def _split(title, file):
    script = SCRIPT.fullmatch(title)
    return (script[1], None) if script else (file, title)


def _tap(lines):
    # Only a TAP stream: indented error text in a spec stream can look like TAP.
    if not lines or lines[0] != TAP_HEADER:
        return
    index = 0
    while index < len(lines):
        point = TAP.fullmatch(lines[index])
        index += 1
        if not point or index >= len(lines) or lines[index] != point[1] + "  ---":
            continue
        # A test point's YAML block can quote any text, re-indented to look
        # like a nested test point; only its own terminator ends it.
        end, block = point[1] + "  ...", []
        index += 1
        while index < len(lines) and lines[index] != end:
            if len(block) < YAML_LINES:
                block.append(lines[index])
            index += 1
        index += 1
        title = point[3]
        if point[2] != "not ok" or not title or DIRECTIVE.search(title):
            continue
        fields = {}
        for detail in block:
            field = re.fullmatch(re.escape(point[1]) + r"  (location|failureType): '(.*)'", detail)
            if field:
                fields.setdefault(field[1], field[2])
        # A suite whose subtests failed is named by those subtests.
        if fields.get("failureType") == "subtestsFailed":
            continue
        location = LOCATION.fullmatch(fields.get("location", ""))
        title = re.sub(r"\\(.)", lambda escaped: "#" if escaped[1] == "#" else " ", title)
        yield _split(title, location[1] if location else None)


def _spec(lines):
    # Node prints its summary once every test has finished. A test's own output,
    # forwarded earlier at column 0, can imitate one, so only the last is read.
    starts = [index for index, line in enumerate(lines) if line == SPEC_HEADER]
    if not starts:
        return
    file = None
    for line in lines[starts[-1] + 1:]:
        if not line or line.startswith(" "):
            continue
        if at := SPEC_AT.fullmatch(line):
            file = at[1]
        elif failed := SPEC_FAIL.fullmatch(line):
            yield _split(failed[1], file)
            file = None
        else:
            return


def _unittest(lines):
    # The runner prints one block per failure and error after every test has
    # run, then its totals. A test's own stderr can imitate a block earlier, so
    # only the last `failures + errors` blocks before the last totals count.
    ran = [index for index in range(1, len(lines)) if lines[index - 1] == SEPARATOR2 and RAN.fullmatch(lines[index])]
    if not ran:
        return
    totals = next((match for line in lines[ran[-1] + 1:ran[-1] + 4] if (match := TOTALS.fullmatch(line))), None)
    if not totals:
        return
    counts = dict(item.split("=") for item in totals[1].split(", "))
    expected = int(counts.get("failures", 0)) + int(counts.get("errors", 0))
    headers = [index for index in range(1, ran[-1]) if lines[index - 1] == SEPARATOR1
               and SEPARATOR2 in lines[index + 1:index + 3] and UNITTEST.fullmatch(lines[index])]
    for index in headers[max(len(headers) - expected, 0):] if expected else ():
        method, path = UNITTEST.fullmatch(lines[index]).groups()
        start = index + 2 + lines[index + 1:index + 3].index(SEPARATOR2)
        file = None
        for detail in lines[start:start + TRACE_LINES]:
            if detail in (SEPARATOR1, SEPARATOR2):
                break
            frame = FRAME.fullmatch(detail)
            if frame and frame[2] == method:
                file = frame[1]
                break
        path = path.removeprefix("__main__.")
        yield file, path if path == method or path.endswith("." + method) else path + "." + method


def _media(lines):
    for line in lines:
        match = MEDIA.fullmatch(line)
        if match and match[3] != "ok":
            yield match[1], match[2] + " " + match[3]


PARSERS = {"tap": _tap, "spec": _spec, "unittest": _unittest, "media": _media}


def _collect(raw, wanted):
    streams = {}
    marker = b'"logs"' if isinstance(raw, bytes) else '"logs"'
    for line in raw.splitlines():
        if marker not in line:
            continue
        try:
            event = json.loads(line)
        except (ValueError, TypeError):
            continue
        records = event.get("logs") if isinstance(event, dict) else None
        if not isinstance(records, list):
            continue
        for record in records:
            if not isinstance(record, dict):
                continue
            vertex, stream, data = record.get("vertex"), record.get("stream"), record.get("data")
            if not (isinstance(vertex, str) and vertex in wanted and type(stream) is int and stream in STREAMS
                    and isinstance(data, str)):
                continue
            try:
                chunk = base64.b64decode(data, validate=True)
            except (binascii.Error, ValueError):
                continue
            streams.setdefault((vertex, stream), Retained()).add(chunk)
    return streams


def failed_tests(raw, failed):
    """Names of failing tests in listed stages; `failed` is [(step label, vertex digest)] in failure order."""
    wanted = {}
    for label, digest in failed:
        step = STEP.fullmatch(label)
        if step and step[1] in STAGES and isinstance(digest, str):
            wanted.setdefault(digest, (label, STAGES[step[1]]))
    if not wanted:
        return []
    streams = _collect(raw, wanted)
    names, seen = [], set()
    for digest, (label, parsers) in wanted.items():
        lines = {}
        for number in STREAMS:
            retained = streams.get((digest, number))
            # Node indents error text only after "\n"; str.splitlines() would
            # also split at "\r", "\f" or U+2028 and expose that text at column 0.
            lines[number] = [ANSI.sub("", line.removesuffix("\r")) for line in retained.text().split("\n")] \
                if retained else []
        for parser in parsers:
            for file, test in PARSERS[parser](lines[STREAM[parser]]):
                entry = {"step": label}
                file = clean_file(file) if file is not None else None
                test = clean_name(test) if test is not None else None
                if file:
                    entry["file"] = file
                if test:
                    entry["test"] = test
                key = (label, file, test)
                if len(entry) == 1 or key in seen:
                    continue
                seen.add(key)
                names.append(entry)
                if len(names) == MAX_NAMES:
                    return names
    return names


def public(row):
    if not (isinstance(row, dict) and set(row) <= {"step", "file", "test"} and len(row) > 1):
        return False
    step = STEP.fullmatch(row["step"]) if isinstance(row.get("step"), str) else None
    file, test = row.get("file"), row.get("test")
    return bool(step and step[1] in STAGES
                and (file is None or isinstance(file, str) and FILE.fullmatch(file) and not secret_like(file))
                and (test is None or isinstance(test, str) and NAME.fullmatch(test) and "::" not in test
                     and not secret_like(test)))


def annotate(component, summary, stream):
    """One annotation per failing test of the build where publication stopped."""
    rows = summary.get("failed_tests") if isinstance(summary, dict) else None
    if not isinstance(rows, list):
        return
    for row in rows[:MAX_NAMES]:
        if not public(row):
            continue
        name = " › ".join(row[key] for key in ("file", "test") if key in row)
        print(f"::error title=Image test failed::{component} {row['step']}: {name}", file=stream, flush=True)
