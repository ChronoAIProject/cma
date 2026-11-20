"""Every public failure code explains itself, and nothing private reaches the explanation."""
import ast
import contextlib
import inspect
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import ci_outcome
from ci_outcome import catalogue, codes, render, report

SCRIPTS = Path(__file__).parent
CATALOGUE_PATH = SCRIPTS / "ci_outcomes.json"
PRIVATE = "private-marker"


def families():
    return catalogue()["families"]


def line_of(module, text, offset=0):
    """The line in a public script holding text, so tests survive unrelated edits."""
    lines = (SCRIPTS / module).read_text(encoding="utf-8").splitlines()
    matches = [number for number, line in enumerate(lines, 1) if text in line]
    assert len(matches) == 1, (module, text)
    return matches[0] + offset


def vocabularies():
    """Map each catalogue family to the exact codes its producers can emit."""
    import staging_cd_allocate as allocate
    import staging_cd_build_cache as build_cache
    import staging_cd_clock as clock
    import staging_cd_component_admission as admission
    import staging_cd_component_report as component_report
    import staging_cd_deploy_result as deploy_result
    import staging_cd_finalize_diagnostics as finalize
    import staging_cd_invoke as invoke
    import staging_cd_publication_diagnostics as publication
    import staging_cd_refusal as refusal
    import staging_cd_runtime as runtime
    import staging_cd_set_report as set_report

    withheld = {reason for _, reason in build_cache.WITHHELD_REASONS}
    withheld |= {build_cache.withheld_reason(subprocess.SubprocessError()), build_cache.withheld_reason(OSError())}
    return {
        "deploy-private": set(refusal.REASONS) | {"unreported", refusal.UNRECOGNIZED},
        "deploy-verification": set(refusal.VERIFICATION_REASONS) | {refusal.UNRECOGNIZED},
        "deploy-precondition": set(invoke.PRECONDITIONS) | {"input"},
        "deploy-component": {reason for reason in deploy_result.STATES.values() if reason != "none"},
        "publication": set(publication.REASONS),
        "publication-stage": set(publication.STAGES),
        "finalize": set(finalize.REASONS.values()) | finalize.FALLBACK_REASONS,
        "finalize-stage": set(finalize.STAGES),
        "allocation": set(allocate.REFUSAL_CODES.values()) | allocate.FALLBACK_CODES,
        "admission": set(admission.REASONS),
        "cache-withheld": withheld,
        "component-report": set(component_report.CODES),
        "deploy-clock": {clock.ADMISSION_FAILURE},
        "component-result": {deploy_result.INVALID_EVIDENCE},
        "release-outcome": set(set_report.OUTCOMES),
        "step": set(runtime.KINDS),
    }


def public_texts(outputs):
    return "\n".join(part for part in outputs if part is not None)


EXPLAINED = re.compile(r"(?P<line>.*?)(?: component=[a-z,-]+| at=(?:\?|(?:[A-Za-z0-9_-]+/){0,2}[A-Za-z0-9_-]+\.py:\d+))*"
                       r" cause=(?:change|transient|operator|unknown)")


def legacy(text):
    """Return a log as its consumers parse it: the original machine lines.

    Every explained line must keep its original prefix, carry only validated
    appended fields and be followed by its annotation; both are removed here.
    """
    kept, lines = [], text.splitlines()
    for index, line in enumerate(lines):
        if line.startswith(("::error ", "::warning ")):
            if not (index and EXPLAINED.fullmatch(lines[index - 1])):
                raise AssertionError("annotation without an explained line: " + line[:80])
            continue
        match = EXPLAINED.fullmatch(line)
        if match:
            if not (index + 1 < len(lines) and lines[index + 1].startswith(("::error ", "::warning "))):
                raise AssertionError("explained line without an annotation: " + line)
            line = match["line"]
        kept.append(line + "\n")
    return "".join(kept)


class CatalogueTests(unittest.TestCase):
    def test_every_code_of_every_vocabulary_has_an_entry(self):
        owned = {family for family, spec in families().items() if "codes" in spec}
        self.assertEqual(set(vocabularies()), owned, "every catalogue family needs a producing vocabulary")
        for family, expected in vocabularies().items():
            with self.subTest(family=family):
                missing = expected - codes(family).keys()
                self.assertFalse(missing, f"{family} codes lack a catalogue entry: {sorted(missing)}")
                self.assertEqual(set(codes(family)), expected, f"{family} catalogues codes nothing emits")

    def test_every_entry_is_complete_one_line_text_with_a_known_cause(self):
        entries = [("uncatalogued", "-", catalogue()["uncatalogued"])]
        for family, spec in families().items():
            entries += [(family, code, value) for code, value in spec.get("codes", {}).items()]
        self.assertGreater(len(entries), 150)
        for family, code, value in entries:
            with self.subTest(family=family, code=code):
                self.assertEqual(set(value), {"title", "meaning", "cause", "next"})
                self.assertIn(value["cause"], ci_outcome.CAUSES)
                for key in ("title", "meaning", "next"):
                    text = value[key]
                    self.assertIsInstance(text, str)
                    self.assertTrue(text.strip(), key)
                    # Leak tests use "private" as their marker; explanations never say it.
                    self.assertNotRegex(text, r"[\r\n{}|]|::|(?i:private)")
                    self.assertLessEqual(len(text), 240)
        self.assertEqual(set(catalogue()["causes"]), set(ci_outcome.CAUSES))

    def test_family_shapes_lines_and_placeholders(self):
        import string
        stage_families = {spec["stages"] for spec in families().values() if "stages" in spec}
        self.assertEqual(set(ci_outcome.LINES), set(families()) - stage_families)
        for family, template in ci_outcome.LINES.items():
            fields = {name for _, name, _, _ in string.Formatter().parse(template) if name is not None}
            self.assertLessEqual(fields, set(ci_outcome.PLACEHOLDERS), family)
            self.assertNotIn("\n", template)
        for family, spec in families().items():
            with self.subTest(family=family):
                self.assertLessEqual(set(spec), {"codes", "codes_from", "stages", "effect", "level", "title",
                                                 "cause", "next"})
                self.assertEqual(("codes" in spec) + ("codes_from" in spec), 1)
                if "codes_from" in spec:
                    self.assertIn("codes", families()[spec["codes_from"]])
                if "stages" in spec:
                    self.assertIn("codes", families()[spec["stages"]])
                self.assertIn(spec.get("level", "error"), ci_outcome.LEVELS)
                self.assertIn(spec.get("cause", "unknown"), ci_outcome.CAUSES)
                if "next" in spec:
                    fields = {name for _, name, _, _ in string.Formatter().parse(spec["next"]) if name is not None}
                    self.assertEqual(fields, {"next"})
                for key in ("effect", "title", "next"):
                    if key in spec:
                        self.assertNotRegex(spec[key].replace("{next}", ""), r"[\r\n{}|]|::|(?i:private)")
                for code in codes(family):
                    self.assertRegex(code, r"\A[A-Za-z][A-Za-z0-9_-]{0,47}\Z")
        self.assertEqual(ci_outcome.COMPONENTS, __import__("staging_cd_targets").TARGETS)

    def test_step_kinds_match_failure_kind_and_the_catalogue(self):
        import staging_cd_runtime as runtime
        returned = set()
        for node in ast.walk(ast.parse(inspect.getsource(runtime.failure_kind))):
            if isinstance(node, ast.Constant) and isinstance(node.value, str) and not node.value.startswith("Classify"):
                returned.add(node.value)
        self.assertEqual(returned | {"declined"}, set(runtime.KINDS))
        self.assertEqual(set(codes("step")), set(runtime.KINDS))

    def test_change_checks_name_existing_literal_checks_of_source_the_change_controls(self):
        # Reservation, receipt and identity checks never blame the change.
        checks = catalogue()["change_checks"]
        self.assertEqual(set(checks), {"staging_cd_build.py", "staging_cd_contract.py"})
        for module, messages in checks.items():
            found = list(ci_outcome.refusal_messages(module).values())
            for message in messages:
                with self.subTest(module=module, message=message):
                    self.assertIn(message, found)
        listed = {message for messages in checks.values() for message in messages}
        for module in ("staging_cd_reservations.py", "staging_cd_receipts.py", "staging_cd_common.py"):
            self.assertFalse(set(ci_outcome.refusal_messages(module).values()) & listed, module)

    def test_catalogue_names_no_url_or_host_other_than_this_repository(self):
        text = CATALOGUE_PATH.read_text(encoding="utf-8")
        for url in re.findall(r"[A-Za-z][A-Za-z0-9+.-]*://[^\s\"]*", text):
            self.assertRegex(url, r"\Ahttps://github\.com/ChronoAIProject/cma(?:/[^\s\"]*)?\Z")
        for token in re.findall(r"[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+", text):
            # Only repository file names may contain a dot; hosts, addresses and
            # registry or cluster names never appear in public explanations.
            self.assertRegex(token, r"\A(?:[A-Za-z0-9_-]+\.(?:py|json|yml|sh|rb)|github\.com|\d+\.\d+)\Z")
        self.assertNotIn("@", text)


class RenderTests(unittest.TestCase):
    def test_existing_machine_line_is_kept_and_validated_fields_are_appended(self):
        dockerfile = line_of("staging_cd_build.py", 'require(value.get("dockerfile_sha256")')
        line, annotation, row = render("publication", "validation_refused", subject="publication", stage="build",
                                       site=f"staging_cd_build.py:{dockerfile}", component="am")
        self.assertEqual(line, "FAIL publication stage=build reason=validation_refused component=am"
                               f" at=staging_cd_build.py:{dockerfile} cause=change")
        self.assertTrue(annotation.startswith(f"::error file=.github/scripts/staging_cd_build.py,line={dockerfile},"
                                              "title=publication%3A Input check refused::"))
        self.assertIn("Next step: Open the reported line", annotation)
        self.assertIn("At this stage: Build the same Dockerfile", annotation)
        self.assertEqual(row.count(" | "), 4)
        line, _, _ = render("allocation", "VERSION_LABEL_REQUIRED", subject="allocation")
        self.assertEqual(line, "FAIL allocation VERSION_LABEL_REQUIRED cause=change")
        line, annotation, _ = render("deploy-release", "pod-restarts", subject="deploy")
        self.assertEqual(line, "FAIL release=pod-restarts recovery=verified cause=change")
        self.assertIn("restored and verified the previous release", annotation)

    def test_a_refusal_blames_the_change_only_at_a_listed_check(self):
        reservation = line_of("staging_cd_reservations.py", "def validate_reservation", 1)
        recipe = line_of("staging_cd_build.py", '"Unknown or retired image component")')
        for site, cause in ((None, "unknown"), (f"staging_cd_reservations.py:{reservation}", "unknown"),
                            (f"staging_cd_build.py:{recipe}", "unknown"),
                            (f"staging_cd_build.py:{line_of('staging_cd_build.py', 'Shared build arguments are invalid') - 1}",
                             "change"),
                            (f"staging_cd_contract.py:{line_of('staging_cd_contract.py', 'declarations disagree') - 1}",
                             "change")):
            with self.subTest(site=site):
                # The build stage alone never makes a generic refusal the change's fault.
                line, _, _ = render("publication", "validation_refused", subject="publication", stage="build", site=site)
                self.assertTrue(line.endswith(" cause=" + cause), line)
        for family, code in (("publication", "io_failed"), ("finalize", "io_failed"), ("step", "os-error"),
                             ("step", "http-client"), ("deploy-private", "image-identity")):
            self.assertTrue(render(family, code, subject="fixture")[0].endswith(" cause=unknown"), (family, code))

    def test_unverified_deployment_is_operator_first_and_names_no_component(self):
        line, annotation, row = render("deploy-verification", "pod-restarts", subject="deploy", recovery="unreported")
        self.assertEqual(line, "FAIL verification=pod-restarts recovery=unreported cause=operator")
        self.assertIn("What changed: Staging may have changed", annotation)
        self.assertIn("title=deploy%3A Deployment unverified%3A Pods restarted", annotation)
        self.assertIn("Next step: An operator reconciles staging first. Then: Check the released components'", annotation)
        self.assertIn("if this change caused it", annotation)
        self.assertIn("| operator | An operator reconciles staging first.", row)
        for code in ("rollout-timeout", "status-pods", "serving", "write-timeout"):
            self.assertTrue(render("deploy-verification", code, subject="deploy")[0].endswith(" cause=operator"))

    def test_values_outside_enums_and_patterns_print_as_unknown(self):
        hostile = PRIVATE + " x::error::y%0A\n|"
        cases = [
            dict(subject=hostile), dict(subject="publication", stage=hostile),
            dict(subject="publication", stage="final_read"),
            dict(subject="publication", site="/etc/" + PRIVATE + ".py:1"),
            dict(subject="publication", site="../" + PRIVATE + ".py:1"),
            dict(subject="publication", site=PRIVATE + ".py:0"),
            dict(subject="publication", component=PRIVATE), dict(subject="publication", component=["am", PRIVATE]),
            dict(subject="publication", component=[]), dict(subject="publication", pr=True),
            dict(subject="publication", pr="12"), dict(subject="publication", pr=-1),
            dict(subject="publication", sha=PRIVATE), dict(subject="publication", sha="A" * 40),
            dict(subject="publication", attempts=0), dict(subject="publication", recovery=hostile),
        ]
        for kwargs in cases:
            with self.subTest(kwargs=kwargs):
                outputs = render("publication", "build_failed", **kwargs)
                text = public_texts(outputs)
                self.assertNotIn(PRIVATE, text)
                self.assertNotIn("/etc/", text)
                self.assertIn("?", outputs[0])
                self.assertTrue(all("\n" not in part and "\r" not in part for part in outputs))
        for family, code in (("publication", PRIVATE), (PRIVATE, "build_failed"), (None, None),
                             ("publication-stage", "build")):
            with self.subTest(family=family, code=code):
                outputs = render(family, code, subject="publication")
                self.assertNotIn(PRIVATE, public_texts(outputs))
                self.assertIn("Uncatalogued failure code", outputs[1])
        line, _, _ = render("publication", "build_failed", subject="publication", pr=2810, attempts=3,
                            sha="0123abc", site="ci_tracks/selection_cache.py:12")
        self.assertTrue(line.endswith(" at=ci_tracks/selection_cache.py:12 pr=2810 sha=0123abc attempts=3 cause=change"))

    def test_annotation_and_summary_escaping(self):
        self.assertEqual(ci_outcome.escape_data("100%\r\n::x"), "100%25%0D%0A::x")
        self.assertEqual(ci_outcome.escape_property("a:b,c%\n"), "a%3Ab%2Cc%25%0A")
        self.assertEqual(ci_outcome.cell("a|b\\c\n d"), "a\\|b\\\\c d")
        _, annotation, row = render("deploy-precondition", "credentials-format", subject="deploy",
                                    site="staging_cd_invoke.py:178")
        head, message = annotation.split("::", 2)[1:]
        self.assertEqual(head.count(","), 2)
        self.assertNotIn(":", head.split("title=", 1)[1])
        self.assertIn("%0A", message)
        self.assertTrue(row.startswith("| deploy | credentials-format: Deployment credentials malformed |"))


class ReportTests(unittest.TestCase):
    def test_report_tables_continue_and_end_with_a_blank_line(self):
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary"
            summary.write_text("Infrastructure revision: fixture\n")
            output = io.StringIO()
            env = {"GITHUB_STEP_SUMMARY": str(summary)}
            first = report("allocation", "READ_FAILED", subject="allocation", stream=output, env=env)
            report("allocation", "DEADLINE_EXCEEDED", subject="allocation", stream=output, env=env)
            lines = output.getvalue().splitlines()
            self.assertEqual(lines[0], first)
            self.assertEqual([line.split(" ", 1)[0] for line in lines], ["FAIL", "::error", "FAIL", "::error"])
            rows = summary.read_text().split("\n")
            self.assertEqual(rows[:4], ["Infrastructure revision: fixture", "", ci_outcome.HEADER, ci_outcome.RULE])
            self.assertTrue(rows[4].startswith("| allocation | READ_FAILED:"))
            self.assertTrue(rows[5].startswith("| allocation | DEADLINE_EXCEEDED:"))
            self.assertEqual(rows[6:], ["", ""])  # A blank line ends the table.
            with summary.open("a") as stream:
                stream.write("Admission-to-result seconds: 1.00\n")
            report("allocation", "READ_FAILED", subject="allocation", stream=output, env=env)
            text = summary.read_text()
            self.assertEqual(text.count(ci_outcome.HEADER), 2)
            self.assertIn("|\n\nAdmission-to-result seconds: 1.00\n\n" + ci_outcome.HEADER, text)
            self.assertTrue(text.endswith(" |\n\n"))

    def test_report_never_raises_and_still_prints_the_machine_line(self):
        with tempfile.TemporaryDirectory() as directory:
            output = io.StringIO()
            env = {"GITHUB_STEP_SUMMARY": directory}  # A directory cannot be appended to.
            line = report("allocation", "READ_FAILED", subject="allocation", stream=output, env=env)
            self.assertEqual(output.getvalue().splitlines()[0], line)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                line = report("allocation", "READ_FAILED", subject="allocation", component=object(), env={})
            self.assertEqual(line, "FAIL allocation READ_FAILED component=? cause=transient")

    def test_a_damaged_catalogue_or_renderer_still_prints_the_original_machine_line(self):
        dockerfile = line_of("staging_cd_build.py", 'require(value.get("dockerfile_sha256")')
        fields = dict(subject="publication", stage="build", site=f"staging_cd_build.py:{dockerfile}", component="am")
        expected = f"FAIL publication stage=build reason=validation_refused component=am at=staging_cd_build.py:{dockerfile}"
        for patched in ("catalogue", "render"):
            with self.subTest(patched=patched), tempfile.TemporaryDirectory() as directory:
                summary = Path(directory) / "summary"
                output = io.StringIO()
                with unittest.mock.patch.object(ci_outcome, patched, side_effect=ValueError(PRIVATE)):
                    line = report("publication", "validation_refused", stream=output,
                                  env={"GITHUB_STEP_SUMMARY": str(summary)}, **fields)
                    self.assertEqual(report("publication", PRIVATE + " x", stream=io.StringIO(), env={},
                                            subject="publication"), "FAIL publication stage=? reason=?")
                    if patched == "catalogue":
                        self.assertIs(ci_outcome.entry("publication", "build_failed"), ci_outcome.UNAVAILABLE)
                        self.assertEqual(ci_outcome.codes("publication"), {})
                self.assertEqual(line, expected)
                self.assertEqual(output.getvalue(), expected + "\n")
                self.assertFalse(summary.exists())

    def test_a_malformed_catalogue_breaks_no_importer_or_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory)
            for name in ("ci_outcome.py", "staging_cd_common.py", "staging_cd_runtime.py"):
                shutil.copy(SCRIPTS / name, copy / name)
            (copy / "ci_outcomes.json").write_text('{"families": ')
            program = (
                "import sys\n"
                "from staging_cd_common import require\n"
                "from staging_cd_runtime import run_private\n"
                "passed = run_private(lambda _m: True, None, subject='fixture-step')\n"
                "failed = run_private(lambda _m: require(False, 'hidden'), None, subject='fixture-step')\n"
                "raise SystemExit(passed * 10 + failed)\n")
            result = subprocess.run([sys.executable, "-B", "-c", program], cwd=copy, capture_output=True, text=True,
                                    env={"PATH": os.environ.get("PATH", ""), "PYTHONDONTWRITEBYTECODE": "1"},
                                    check=False)
        self.assertEqual(result.returncode, 1, result.stderr)
        # Only the plain machine lines: no annotation, no cause, the same exit status.
        self.assertEqual(result.stdout, "PASS fixture-step\nFAIL fixture-step reason=refused\n")
        self.assertEqual(result.stderr, "")
        with self.assertRaises(ValueError):
            ci_outcome.check_catalogue({"families": {}})

    def test_error_site_names_public_frames_and_never_the_message(self):
        def fail():
            raise KeyError(PRIVATE)
        raised_at = inspect.currentframe().f_lineno - 1
        try:
            fail()
        except KeyError as error:
            self.assertEqual(ci_outcome.error_site(error), f"staging_cd_outcome_tests.py:{raised_at}")
        try:
            raise KeyError(PRIVATE)
        except KeyError as error:
            self.assertEqual(ci_outcome.error_site(error), f"staging_cd_outcome_tests.py:{raised_at + 7}")
        namespace = {}
        exec(compile("def outside():\n    raise ValueError('x')\n", "/elsewhere/" + PRIVATE + ".py", "exec"), namespace)
        try:
            namespace["outside"]()
        except ValueError as error:
            # Only the catching frame is public, and it names a call, not the failure.
            self.assertIsNone(ci_outcome.error_site(error))
        error = ValueError()
        error.site = "../" + PRIVATE + ".py:1"
        self.assertIsNone(ci_outcome.error_site(error))


def refusal_calls():
    """Yield (module, line, message node, module constants) for each refusal message."""
    sources = sorted(path for path in Path(__file__).parent.glob("*.py")
                     if not path.name.endswith(("_tests.py", "-tests.py", "_fixture.py", "_fixtures.py")))
    trees = {path: ast.parse(path.read_text(encoding="utf-8")) for path in sources}
    users = {path: tree for path, tree in trees.items() if path.name == "staging_cd_common.py"
             or any(isinstance(node, ast.ImportFrom) and node.module == "staging_cd_common" for node in ast.walk(tree))}
    classes = [node for tree in users.values() for node in ast.walk(tree) if isinstance(node, ast.ClassDef)]
    refusals, grew = {"Refusal"}, True
    while grew:
        found = {node.name for node in classes if any(isinstance(base, ast.Name) and base.id in refusals
                                                      for base in node.bases)}
        grew, refusals = not found <= refusals, refusals | found
    # Subclasses with their own constructor are checked where they call the
    # base constructor; every other refusal class is checked at each call.
    custom = {node.name for node in classes if node.name in refusals - {"Refusal"}
              and any(isinstance(item, ast.FunctionDef) and item.name == "__init__" for item in node.body)}
    for path, tree in users.items():
        constants = {target.id: node.value for node in tree.body if isinstance(node, ast.Assign)
                     for target in node.targets if isinstance(target, ast.Name)}
        # require() itself forwards its caller's message; its callers are checked.
        forwarding = {id(inner) for node in tree.body if isinstance(node, ast.FunctionDef)
                      and path.name == "staging_cd_common.py" and node.name == "require" for inner in ast.walk(node)}
        for node in ast.walk(tree):
            if id(node) in forwarding:
                continue
            if isinstance(node, ast.ClassDef) and node.name in custom:
                for call in ast.walk(node):
                    if isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute) \
                            and call.func.attr == "__init__":
                        yield path.name, call.lineno, call.args[0] if len(call.args) == 1 else None, constants
            elif isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
                if node.func.id == "require":
                    yield path.name, node.lineno, node.args[1] if len(node.args) == 2 else None, constants
                elif node.func.id in refusals - custom:
                    yield path.name, node.lineno, node.args[0] if len(node.args) == 1 else None, constants


class DiagnosticReportTests(unittest.TestCase):
    """Publication, finalization and allocation failures name their check and next step."""

    def explained(self, run):
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary"
            output = io.StringIO()
            with unittest.mock.patch.dict("os.environ", {"GITHUB_STEP_SUMMARY": str(summary)}), \
                    contextlib.redirect_stdout(output):
                self.assertEqual(run(), 1)
            first, line, annotation = output.getvalue().splitlines()
            self.assertNotIn(PRIVATE, output.getvalue() + summary.read_text())
            self.assertEqual(summary.read_text().split("\n")[:2], [ci_outcome.HEADER, ci_outcome.RULE])
            self.assertEqual(summary.read_text().split("\n")[3:], ["", ""])
            return first, line, annotation

    def test_publication_reports_stage_component_and_reservation_check(self):
        from staging_cd_publication_diagnostics import run_publication
        from staging_cd_reservations import validate_reservation
        site = line_of("staging_cd_reservations.py", "def validate_reservation", 1)

        def operation(_masker, diagnostics):
            diagnostics.mark("build")
            validate_reservation({"schema": PRIVATE})
        first, line, annotation = self.explained(lambda: run_publication(operation, component="am"))
        self.assertEqual(first, "FAIL publication")
        # A reservation refusal is never blamed on the change, whatever the stage.
        self.assertEqual(line, "FAIL publication stage=build reason=validation_refused component=am"
                               f" at=staging_cd_reservations.py:{site} cause=unknown")
        self.assertTrue(annotation.startswith(f"::error file=.github/scripts/staging_cd_reservations.py,line={site},"))
        self.assertIn("Stage build: builds the component image", annotation)

    def test_finalization_reports_stage_component_and_check(self):
        from staging_cd_common import require
        from staging_cd_finalize_diagnostics import run_finalization

        def operation(_masker, diagnostics):
            diagnostics.mark("tag_push")
            require(False, "Final annotation could not be created")
        raised_at = inspect.getsourcelines(operation)[1] + 2
        _, line, annotation = self.explained(lambda: run_finalization(operation, component="web"))
        self.assertEqual(line, "FAIL finalize stage=tag_push reason=annotation_failed component=web"
                               f" at=staging_cd_outcome_tests.py:{raised_at} cause=transient")
        self.assertIn("Next step: Re-run the failed jobs.", annotation)

    def test_allocation_reports_the_code_and_its_check(self):
        import staging_cd_allocate as allocation
        line_of = next(number for number, text in enumerate(
            Path(__file__).with_name("staging_cd_allocate.py").read_text().splitlines(), 1)
            if '"Release queue is invalid")' in text) - 1
        _, line, annotation = self.explained(lambda: allocation.run_allocation(
            lambda _masker: allocation.allocate_queue({"head": PRIVATE}, None, None, 1, 1)))
        self.assertEqual(line, f"FAIL allocation QUEUE_INVALID at=staging_cd_allocate.py:{line_of} cause=unknown")
        self.assertIn("title=allocation%3A Release queue invalid", annotation)


class FixedCodeReportTests(unittest.TestCase):
    """Fixed deployment codes keep their lines and gain a check, cause and next step."""

    def run_main(self, main, env):
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary"
            output = io.StringIO()
            with unittest.mock.patch.dict("os.environ", dict(env, GITHUB_STEP_SUMMARY=str(summary)), clear=True), \
                    contextlib.redirect_stdout(output):
                code = main()
            text = output.getvalue()
            self.assertNotIn(PRIVATE, text + (summary.read_text() if summary.exists() else ""))
            return code, text, summary.read_text() if summary.exists() else ""

    def test_expired_admission_names_the_bound_that_refused(self):
        import staging_cd_clock as clock
        source = Path(clock.__file__).read_text().splitlines()
        line = next(n for n, text in enumerate(source, 1) if '"Deployment admission timestamp is malformed")' in text) - 1
        code, text, summary = self.run_main(clock.main, {"CD_STARTED_AT": PRIVATE})
        self.assertEqual(code, 30)
        self.assertEqual(legacy(text), "FAIL: deployment not admitted; invalid or expired admission time\n")
        self.assertTrue(text.startswith("FAIL: deployment not admitted; invalid or expired admission time"
                                        f" at=staging_cd_clock.py:{line} cause=transient\n"))
        self.assertIn("What changed: Nothing.", text)
        self.assertIn("| deploy-admission | admission-time:", summary)

    def test_invalid_component_result_names_its_check(self):
        import staging_cd_deploy_result as deploy_result
        code, text, _ = self.run_main(deploy_result.main, {"CD_COMPONENT_RESULT": PRIVATE})
        self.assertEqual(code, 1)
        self.assertEqual(legacy(text), "FAIL component-result reason=invalid-public-evidence\n")
        self.assertRegex(text.splitlines()[0], r" at=staging_cd_\w+\.py:\d+ cause=unknown\Z")

    def test_redeploy_outcome_explains_each_component_and_the_run(self):
        import staging_cd_set_report as set_report
        env = {"CD_ADMISSION_JOB_RESULT": "success", "CD_DEPLOY_JOB_RESULT": "skipped", "CD_SET_EXPECTED": "true",
               "CD_PUBLICATION_JOB_RESULT": "failure",
               "CD_COMPONENT_FAILURES": '{"runtime":"publication-unqualified","web":"superseded-source"}'}
        code, text, summary = self.run_main(set_report.main, env)
        self.assertEqual(code, 1)
        lines = [line for line in legacy(text).splitlines() if not line.startswith("CD_RESULT ")]
        self.assertEqual(lines, [
            "FAIL component=runtime reason=publication-unqualified",
            "FAIL component=web reason=superseded-source",
            "FAIL: publication or qualification failed for one or more components; consult admitted deployment outcome",
            "::error::One or more release outcomes failed. Maintainer reconciliation is required.",
            "FAIL release-outcome reason=publication-failed"])
        self.assertIn("FAIL component=web reason=superseded-source cause=transient", text)
        self.assertEqual(summary.count("| redeploy-outcome |"), 3)
        code, text, _ = self.run_main(set_report.main, dict(env, CD_COMPONENT_FAILURES=PRIVATE))
        self.assertEqual(code, 1)
        self.assertEqual(legacy(text).splitlines(), [
            "::error::Component outcome evidence is invalid; no deployment acceptance is claimed.",
            "FAIL release-outcome reason=evidence-invalid"])


class RefusalSiteTests(unittest.TestCase):
    def test_refusal_names_the_checking_line_and_never_its_message(self):
        from staging_cd_common import Refusal, require
        from staging_cd_publication_diagnostics import ImageCommandRefusal
        from staging_cd_reservations import ReservationSuperseded
        here = Path(__file__).name
        cases = (
            (lambda: require(False, PRIVATE), inspect.currentframe().f_lineno),
            (lambda: (_ for _ in ()).throw(Refusal(PRIVATE)), inspect.currentframe().f_lineno),
            (lambda: (_ for _ in ()).throw(ReservationSuperseded()), inspect.currentframe().f_lineno),
            (lambda: (_ for _ in ()).throw(ImageCommandRefusal(["docker"], b"")), inspect.currentframe().f_lineno),
        )
        for raise_it, line in cases:
            with self.subTest(line=line), self.assertRaises(Refusal) as raised:
                raise_it()
            self.assertEqual(raised.exception.site, f"{here}:{line}")
            self.assertEqual(ci_outcome.error_site(raised.exception), f"{here}:{line}")
            self.assertNotIn(PRIVATE, raised.exception.site)

    def test_a_check_in_another_module_names_that_module_and_line(self):
        from staging_cd_clock import elapsed
        from staging_cd_common import Refusal
        source = Path(__file__).with_name("staging_cd_clock.py").read_text().splitlines()
        line = next(number for number, text in enumerate(source, 1) if "Deployment admission timestamp is malformed" in text)
        with self.assertRaises(Refusal) as raised:
            elapsed(PRIVATE)
        self.assertIn(raised.exception.site, {f"staging_cd_clock.py:{line}", f"staging_cd_clock.py:{line - 1}"})

    def test_every_require_and_refusal_message_is_a_string_literal(self):
        # A literal message keeps each refusal a fixed, reviewable condition:
        # the reported module.py:LINE then identifies it without printing it.
        offenders, checked = [], 0
        for module, line, message, constants in refusal_calls():
            checked += 1
            value = constants.get(message.id) if isinstance(message, ast.Name) else message
            if not (isinstance(value, ast.Constant) and isinstance(value.value, str)):
                offenders.append(f"{module}:{line}")
        self.assertGreater(checked, 400)
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
