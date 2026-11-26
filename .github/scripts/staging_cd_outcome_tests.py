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


def script_sources():
    """Every public script that can report a failure or refuse an input."""
    return {path.name: path.read_text(encoding="utf-8") for path in sorted(SCRIPTS.glob("*.py"))
            if not path.name.endswith(("_tests.py", "-tests.py", "_fixture.py", "_fixtures.py"))}


def line_of(module, text, offset=0):
    """The line in a public script holding text, so tests survive unrelated edits."""
    lines = (SCRIPTS / module).read_text(encoding="utf-8").splitlines()
    matches = [number for number, line in enumerate(lines, 1) if text in line]
    assert len(matches) == 1, (module, text)
    return matches[0] + offset


def guarded_vocabularies():
    """Codes that reach report() through a variable, by the vocabulary that guards them."""
    import staging_cd_allocate as allocate
    import staging_cd_build_cache as build_cache
    import staging_cd_component_admission as admission
    import staging_cd_finalize_diagnostics as finalize
    import staging_cd_invoke as invoke
    import staging_cd_publication_diagnostics as publication
    import staging_cd_refusal as refusal

    withheld = {reason for _, reason in build_cache.WITHHELD_REASONS}
    withheld |= {build_cache.withheld_reason(subprocess.SubprocessError()), build_cache.withheld_reason(OSError())}
    return {
        ("staging_cd_publication_diagnostics.py", "publication"): set(publication.REASONS),
        ("staging_cd_finalize_diagnostics.py", "finalize"): set(finalize.REASONS.values()) | finalize.FALLBACK_REASONS,
        ("staging_cd_invoke.py", "deploy-verification"): set(refusal.VERIFICATION_REASONS) | {refusal.UNRECOGNIZED},
        ("staging_cd_invoke.py", "deploy-release"): set(refusal.VERIFICATION_REASONS) | {refusal.UNRECOGNIZED},
        ("staging_cd_invoke.py", "deploy-private"): set(refusal.REASONS) | {refusal.UNRECOGNIZED},
        ("staging_cd_invoke.py", "deploy-precondition"): set(invoke.PRECONDITIONS),
        ("staging_cd_set_report.py", "admission"): set(admission.REASONS),
        ("staging_cd_publish.py", "cache-withheld"): withheld,
        # The settlement gate reports the transient code its allocation step recorded.
        ("staging_cd_allocate.py", "allocation"): set(allocate.TRANSIENT_CODES) | {"UNCLASSIFIED_REFUSAL"},
    }


def explained_vocabularies():
    """Codes looked up with entry() rather than reported: stages and report columns."""
    import staging_cd_component_admission as admission
    import staging_cd_component_report as component_report
    import staging_cd_deploy_result as deploy_result
    import staging_cd_finalize_diagnostics as finalize
    import staging_cd_publication_diagnostics as publication
    return {
        "publication-stage": set(publication.STAGES),
        "finalize-stage": set(finalize.STAGES),
        "component-report": set(component_report.CODES),
        "deploy-component": {reason for reason in deploy_result.STATES.values() if reason != "none"},
        "admission": set(admission.REASONS),
    }


def reporter_calls(sources):
    """Yield (module, line, family, codes, kind) for every report() call.

    A code is followed through literals, module constants, lookups in a module
    constant dict, assignments in the enclosing function (including tuple
    unpacking and conditional expressions) and the literal returns of module
    functions it calls. kind is None when every code is a literal, "variable"
    when a code is a parameter, attribute or keyed item that a named vocabulary
    must guard, and "derived" when a code is computed, which is never allowed.
    """
    for module, source in sources.items():
        tree = ast.parse(source)
        imported = any(isinstance(node, ast.ImportFrom) and node.module == "ci_outcome"
                       and any(alias.name == "report" and alias.asname is None for alias in node.names)
                       for node in ast.walk(tree))
        constants = {target.id: node.value for node in tree.body if isinstance(node, ast.Assign)
                     for target in node.targets if isinstance(target, ast.Name)}
        functions = {node.name: node for node in tree.body if isinstance(node, ast.FunctionDef)}
        enclosing = {}
        for function in ast.walk(tree):
            if isinstance(function, (ast.FunctionDef, ast.Lambda)):
                for inner in ast.walk(function):
                    enclosing.setdefault(id(inner), function)  # Outermost function wins.

        def merge(*results):
            found, kinds = set(), {kind for _, kind in results} - {None}
            for codes_found, _ in results:
                found |= codes_found
            return found, "derived" if "derived" in kinds else ("variable" if kinds else None)

        def values(node, scope, seen=frozenset()):
            if isinstance(node, ast.Constant) and isinstance(node.value, str):
                return {node.value}, None
            if isinstance(node, ast.IfExp):
                return merge(values(node.body, scope, seen), values(node.orelse, scope, seen))
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id in functions:
                returns = [item.value for item in ast.walk(functions[node.func.id])
                           if isinstance(item, ast.Return) and item.value is not None]
                return merge(*(values(value, functions[node.func.id], seen) for value in returns))
            if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "get"
                    and isinstance(node.func.value, ast.Name)
                    and isinstance(constants.get(node.func.value.id), ast.Dict) and len(node.args) == 2):
                table = constants[node.func.value.id]
                return merge(*(values(value, scope, seen) for value in table.values), values(node.args[1], scope, seen))
            if isinstance(node, ast.Attribute) or (isinstance(node, ast.Subscript) and isinstance(node.slice, ast.Constant)):
                return set(), "variable"
            if isinstance(node, ast.Name) and node.id not in seen:
                assigned = []
                for item in ast.walk(scope) if scope is not None else ():
                    if not isinstance(item, ast.Assign):
                        continue
                    for target in item.targets:
                        if isinstance(target, ast.Name) and target.id == node.id:
                            assigned.append(item.value)
                        elif isinstance(target, ast.Tuple) and isinstance(item.value, ast.Tuple):
                            assigned += [value for name, value in zip(target.elts, item.value.elts)
                                         if isinstance(name, ast.Name) and name.id == node.id]
                if not assigned and node.id in constants:
                    assigned = [constants[node.id]]
                if not assigned:
                    return set(), "variable"  # A parameter or loop variable.
                return merge(*(values(value, scope, seen | {node.id}) for value in assigned))
            return set(), "derived"

        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            direct = imported and isinstance(node.func, ast.Name) and node.func.id == "report"
            qualified = (isinstance(node.func, ast.Attribute) and node.func.attr == "report"
                         and isinstance(node.func.value, ast.Name) and node.func.value.id == "ci_outcome")
            if not (direct or qualified):
                continue
            family = node.args[0] if node.args else None
            family = family.value if isinstance(family, ast.Constant) and isinstance(family.value, str) else None
            found, kind = values(node.args[1], enclosing.get(id(node))) if len(node.args) > 1 else (set(), "derived")
            yield module, node.lineno, family, found, kind


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
    def test_every_reported_code_is_catalogued_and_every_catalogued_code_is_emitted(self):
        import staging_cd_allocate as allocate
        guarded, emitted, problems, used = guarded_vocabularies(), {}, [], set()
        for module, line, family, found, kind in reporter_calls(script_sources()):
            if family not in ci_outcome.LINES or family not in families():
                problems.append(f"{module}:{line} reports an unknown or non-literal family {family!r}")
                continue
            missing = found - codes(family).keys()
            if missing:
                problems.append(f"{module}:{line} reports uncatalogued {family} codes {sorted(missing)}")
            if kind == "derived":
                problems.append(f"{module}:{line} reports a computed {family} code")
            elif kind == "variable":
                used.add((module, family))
                if (module, family) not in guarded:
                    problems.append(f"{module}:{line} reports an unguarded {family} code")
            emitted.setdefault(family, set()).update(found)
        self.assertEqual(problems, [])
        self.assertEqual(used, set(guarded), "every guarded vocabulary must still guard a variable code")
        self.assertLessEqual(set(allocate.REFUSAL_CODES.values()) | allocate.FALLBACK_CODES, emitted["allocation"])
        for (module, family), vocabulary in guarded.items():
            emitted.setdefault(family, set()).update(vocabulary)
        for family, vocabulary in explained_vocabularies().items():
            emitted.setdefault(family, set()).update(vocabulary)
        owned = {family for family, spec in families().items() if "codes" in spec}
        self.assertEqual(set(emitted) - {"deploy-release"}, owned, "every catalogue family needs a producer")
        for family in sorted(owned):
            with self.subTest(family=family):
                self.assertEqual(set(codes(family)), emitted[family], f"{family} catalogue differs from its producers")

    def test_reporter_check_fails_for_a_renamed_unguarded_or_computed_code(self):
        source = '''
from ci_outcome import report
CODE = "deployment-not-accepted"
TABLE = {"Fixed message": "evidence-invalid"}
def explained(flag, token, metrics, error):
    outcome = "evidence-invalid" if flag else "deployment-unaccepted"
    report("release-outcome", outcome, subject="x")
    report("release-outcome", CODE, subject="x")
    report("admission", token, subject="x")
    report(FAMILY, "evidence-invalid", subject="x")
    report("release-outcome", TABLE.get(str(error), "publication-failed"), subject="x")
    report("cache-withheld", metrics["reason"], subject="x")
    report("cache-withheld", metrics["reason"] + "x", subject="x")
    report("cache-withheld", f"{token}", subject="x")
    report("deploy-private", token[len("private-"):], subject="x")
'''
        calls = list(reporter_calls({"fixture.py": source}))
        self.assertEqual([(family, found, kind) for _, _, family, found, kind in calls], [
            ("release-outcome", {"evidence-invalid", "deployment-unaccepted"}, None),
            ("release-outcome", {"deployment-not-accepted"}, None),
            ("admission", set(), "variable"),
            (None, {"evidence-invalid"}, None),
            ("release-outcome", {"evidence-invalid", "publication-failed"}, None),
            ("cache-withheld", set(), "variable"),
            ("cache-withheld", set(), "derived"),
            ("cache-withheld", set(), "derived"),
            ("deploy-private", set(), "derived")])
        self.assertEqual(calls[0][3] - codes("release-outcome").keys(), {"deployment-unaccepted"})

    def test_step_kinds_match_failure_kind_and_the_catalogue(self):
        import staging_cd_runtime as runtime
        returned = set()
        for node in ast.walk(ast.parse(inspect.getsource(runtime.failure_kind))):
            if isinstance(node, ast.Constant) and isinstance(node.value, str) and not node.value.startswith("Classify"):
                returned.add(node.value)
        self.assertEqual(returned | {"declined"}, set(runtime.KINDS))
        self.assertEqual(set(codes("step")), set(runtime.KINDS))

    def test_every_entry_is_complete_one_line_text_with_a_known_cause(self):
        entries = [("uncatalogued", "-", catalogue()["uncatalogued"]), ("unavailable", "-", ci_outcome.UNAVAILABLE)]
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
                             ("step", "http-client"), ("deploy-private", "image-identity"),
                             ("deploy-private", "compatibility"), ("deploy-release", "rollout-timeout")):
            self.assertTrue(render(family, code, subject="fixture")[0].endswith(" cause=unknown"), (family, code))

    def test_unverified_deployment_is_operator_first_and_names_no_component(self):
        line, annotation, row = render("deploy-verification", "pod-restarts", subject="deploy", recovery="unreported")
        self.assertEqual(line, "FAIL verification=pod-restarts recovery=unreported cause=operator")
        self.assertIn("What changed: Staging may have changed", annotation)
        self.assertIn("title=deploy%3A Deployment unverified%3A Pods restarted", annotation)
        self.assertIn("Next step: An operator reconciles staging first. Then: Check the restarted pods' logs", annotation)
        self.assertIn("if this change caused it", annotation)
        self.assertIn("| operator | An operator reconciles staging first.", row)
        self.assertTrue(render("deploy-precondition", "mode", subject="deploy")[0].endswith(" cause=operator"))
        for code in ("rollout-timeout", "status-pods", "serving", "write-timeout"):
            self.assertTrue(render("deploy-verification", code, subject="deploy")[0].endswith(" cause=operator"))

    def test_specific_restart_reasons_keep_verification_operator_first(self):
        cases = (
            ("pod-restarts-membership", "membership or ownership renewal", "change"),
            ("pod-restarts-startup-gate", "active warm-pool readiness", "operator"),
            ("pod-restarts-panic", "inspect the panic", "change"),
            ("pod-restarts", "restarted pods' logs", "change"),
            ("unrecognized", "Add the controller's new reason", "operator"),
        )
        for reason, instruction, cause in cases:
            with self.subTest(reason=reason):
                line, annotation, row = render("deploy-verification", reason, subject="deploy", recovery="unreported")
                self.assertEqual(line, f"FAIL verification={reason} recovery=unreported cause=operator")
                self.assertIn("An operator reconciles staging first.", annotation)
                self.assertIn(instruction, annotation)
                self.assertIn(instruction, row)
                line, annotation, _ = render("deploy-release", reason, subject="deploy")
                self.assertEqual(line, f"FAIL release={reason} recovery=verified cause={cause}")
                self.assertIn(instruction, annotation)

    def test_a_caller_may_lower_an_annotation_to_a_warning_but_not_invent_a_level(self):
        line, annotation, _ = render("deploy-private", "clock", subject="deploy", level="warning")
        self.assertEqual(line, "FAIL precondition=private-clock cause=transient")
        self.assertTrue(annotation.startswith("::warning "), annotation)
        for level in (None, "notice", "error\n::add-mask::x"):
            _, annotation, _ = render("deploy-private", "clock", subject="deploy", level=level)
            self.assertTrue(annotation.startswith("::error "), annotation)

    def test_an_abandoned_apply_names_the_stored_generation_and_the_operator_fix(self):
        # chrono-cd stored a generation and never applied it. No rollout is
        # running, so "re-run after it settles" would send the owner in circles.
        import staging_cd_refusal as refusal
        self.assertIn("apply-abandoned", refusal.REASONS)
        self.assertIn("apply-abandoned", refusal.VERIFICATION_REASONS)
        fix = "the deployment service's admin reconcile for"
        pointer = "(see the cma-infra staging README)"
        line, annotation, row = render("deploy-private", "apply-abandoned", subject="deploy")
        self.assertEqual(line, "FAIL precondition=private-apply-abandoned cause=operator")
        self.assertIn("What changed: Nothing.", annotation)
        # The stuck app can be a dependency the release does not deploy.
        self.assertIn("one of the staging apps this release depends on or deploys", annotation)
        self.assertIn("never applied", annotation)
        self.assertNotIn("settles", annotation)
        for text in (annotation, row):
            self.assertIn(fix, text)
            self.assertIn(pointer, text)
            # A re-submit stores a new generation; it cannot re-send the old one.
            self.assertIn("re-submits the same spec", text)
        line, annotation, row = render("deploy-verification", "apply-abandoned", subject="deploy",
                                       recovery="unreported")
        self.assertEqual(line, "FAIL verification=apply-abandoned recovery=unreported cause=operator")
        self.assertIn("never applied", annotation)
        self.assertIn("Next step: An operator reconciles staging first. Then: ", annotation)
        for text in (annotation, row):
            self.assertIn(fix, text)
            self.assertIn(pointer, text)
            self.assertIn("re-submit the same spec", text)
            # The reconcile applies this run's unverified write; only a re-run verifies it.
            self.assertIn("re-run this deployment so it is verified", text)
        line, annotation, _ = render("deploy-release", "apply-abandoned", subject="deploy")
        self.assertEqual(line, "FAIL release=apply-abandoned recovery=verified cause=operator")

    def test_the_catalogue_publishes_no_deployment_service_route(self):
        # The exact admin route stays in the private deployment README.
        text = CATALOGUE_PATH.read_text(encoding="utf-8")
        self.assertNotIn("/api/", text)
        self.assertNotRegex(text, r"\b(?:POST|PUT|PATCH|DELETE) /")

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

    def test_a_crlf_or_multibyte_summary_keeps_its_bytes_and_one_table(self):
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary"
            before = "Revision: fixture \u00e9\r\nSeconds: 1.00\r\n".encode("utf-8")
            summary.write_bytes(before)
            env = {"GITHUB_STEP_SUMMARY": str(summary)}
            for code in ("READ_FAILED", "DEADLINE_EXCEEDED", "WRITE_UNCERTAIN"):
                report("allocation", code, subject="allocation", stream=io.StringIO(), env=env)
            data = summary.read_bytes()
        self.assertTrue(data.startswith(before + b"\n" + ci_outcome.HEADER.encode() + b"\n"))
        rows = data[len(before):].decode("utf-8").split("\n")
        self.assertEqual([row.split(" | ")[1].split(":")[0] for row in rows[3:6]],
                         ["READ_FAILED", "DEADLINE_EXCEEDED", "WRITE_UNCERTAIN"])
        self.assertEqual(rows[6:], ["", ""])

    def test_retry_lines_cross_the_boundary_with_fixed_fields_only(self):
        program = (
            "import ci_retry\n"
            "from staging_cd_runtime import run_private\n"
            "calls = []\n"
            "def flaky():\n"
            "    calls.append(1)\n"
            "    if len(calls) < 2:\n"
            "        raise ConnectionResetError('hidden-detail')\n"
            "    return True\n"
            "def read(_m):\n"
            "    return ci_retry.call(flaky, name='fixture-read', classify=ci_retry.classify, sleep=lambda _s: None,\n"
            "                         rand=lambda: 0.0)\n"
            "def exhausted(_m):\n"
            "    return ci_retry.call(lambda: (_ for _ in ()).throw(ConnectionResetError('hidden-detail')),\n"
            "                         name='fixture-read', classify=ci_retry.classify, attempts=2,\n"
            "                         sleep=lambda _s: None, rand=lambda: 0.0)\n"
            "raise SystemExit(run_private(read, None, subject='fixture-step') * 10\n"
            "                 + run_private(exhausted, None, subject='fixture-step'))\n")
        result = subprocess.run([sys.executable, "-B", "-c", program], cwd=SCRIPTS, capture_output=True, text=True,
                                env={"PATH": os.environ.get("PATH", ""), "PYTHONDONTWRITEBYTECODE": "1"}, check=False)
        self.assertEqual(result.returncode, 1, result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(lines[:3], ["RETRY op=fixture-read attempt=2/3 wait=0.0s", "PASS fixture-step",
                                     "RETRY op=fixture-read attempt=2/2 wait=0.0s"])
        self.assertEqual(legacy(result.stdout).splitlines()[3], "FAIL fixture-step reason=network")
        self.assertNotIn("hidden-detail", result.stdout + result.stderr)

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


def refusal_calls(sources):
    """Yield (module, line, message node, module constants) for each refusal message.

    Calls are found by name and by module attribute, including aliases of
    staging_cd_common and of its require and Refusal.
    """
    trees = {name: ast.parse(source) for name, source in sources.items()}

    def uses_common(tree):
        return any((isinstance(node, ast.ImportFrom) and node.module == "staging_cd_common")
                   or (isinstance(node, ast.Import) and any(alias.name == "staging_cd_common" for alias in node.names))
                   for node in ast.walk(tree))
    users = {name: tree for name, tree in trees.items() if name == "staging_cd_common.py" or uses_common(tree)}
    classes = [node for tree in users.values() for node in ast.walk(tree) if isinstance(node, ast.ClassDef)]
    refusals, grew = {"Refusal"}, True
    while grew:
        found = {node.name for node in classes if any(
            (isinstance(base, ast.Name) and base.id in refusals) or (isinstance(base, ast.Attribute) and base.attr in refusals)
            for base in node.bases)}
        grew, refusals = not found <= refusals, refusals | found
    # Subclasses with their own constructor are checked where they call the
    # base constructor; every other refusal class is checked at each call.
    custom = {node.name for node in classes if node.name in refusals - {"Refusal"}
              and any(isinstance(item, ast.FunctionDef) and item.name == "__init__" for item in node.body)}
    grew = True
    while grew:
        inherited = {node.name for node in classes if any(
            (isinstance(base, ast.Name) and base.id in custom) or (isinstance(base, ast.Attribute) and base.attr in custom)
            for base in node.bases)}
        grew, custom = not inherited <= custom, custom | inherited
    for module, tree in users.items():
        constants = {target.id: node.value for node in tree.body if isinstance(node, ast.Assign)
                     for target in node.targets if isinstance(target, ast.Name)}
        requires, classes_here, modules = {"require"}, set(refusals - custom), {"staging_cd_common"}
        for node in ast.walk(tree):
            if isinstance(node, ast.ImportFrom) and node.module == "staging_cd_common":
                for alias in node.names:
                    if alias.name == "require":
                        requires.add(alias.asname or alias.name)
                    elif alias.name in refusals - custom:
                        classes_here.add(alias.asname or alias.name)
            elif isinstance(node, ast.Import):
                modules |= {alias.asname or alias.name for alias in node.names if alias.name == "staging_cd_common"}
        # require() itself forwards its caller's message; its callers are checked.
        forwarding = {id(inner) for node in tree.body if isinstance(node, ast.FunctionDef)
                      and module == "staging_cd_common.py" and node.name == "require" for inner in ast.walk(node)}
        for node in ast.walk(tree):
            if id(node) in forwarding:
                continue
            if isinstance(node, ast.ClassDef) and node.name in custom:
                for call in ast.walk(node):
                    if isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute) \
                            and call.func.attr == "__init__":
                        yield module, call.lineno, call.args[0] if len(call.args) == 1 else None, constants
                continue
            if not isinstance(node, ast.Call):
                continue
            func = node.func
            name = func.id if isinstance(func, ast.Name) else None
            via_module = isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name) \
                and func.value.id in modules
            if name in requires or (via_module and func.attr == "require"):
                yield module, node.lineno, node.args[1] if len(node.args) == 2 else None, constants
            elif name in classes_here or (isinstance(func, ast.Attribute) and func.attr in refusals - custom):
                yield module, node.lineno, node.args[0] if len(node.args) == 1 else None, constants


def literal_offenders(sources):
    offenders, checked = [], 0
    for module, line, message, constants in refusal_calls(sources):
        checked += 1
        value = constants.get(message.id) if isinstance(message, ast.Name) else message
        if not (isinstance(value, ast.Constant) and isinstance(value.value, str)):
            offenders.append(f"{module}:{line}")
    return offenders, checked


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

    def test_registry_copy_readback_refusals_name_their_outcome(self):
        from staging_cd_common import Refusal
        from staging_cd_publication_diagnostics import run_publication
        for message, reason, cause in (
                ("Registry digest differs from the scanned archive", "registry_digest_conflict", "operator"),
                ("Registry publication outcome is uncertain", "registry_write_uncertain", "unknown"),
                ("Registry copy budget expired", "command_timeout", "transient")):
            with self.subTest(reason=reason):
                def operation(_masker, diagnostics, message=message):
                    diagnostics.mark("registry_copy")
                    raise Refusal(message)
                _, line, annotation = self.explained(lambda: run_publication(operation, component="eg"))
                self.assertRegex(line, rf"\AFAIL publication stage=registry_copy reason={reason} component=eg"
                                       rf" at=staging_cd_outcome_tests\.py:\d+ cause={cause}\Z")
                self.assertIn("Next step: ", annotation)

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
        site = line_of("staging_cd_allocate.py", 'and isinstance(queue.get("changes"), list), "Release queue is invalid")', -1)
        _, line, annotation = self.explained(lambda: allocation.run_allocation(
            lambda _masker: allocation.allocate_queue({"head": PRIVATE}, None, None, 1, 1)))
        self.assertEqual(line, f"FAIL allocation QUEUE_INVALID at=staging_cd_allocate.py:{site} cause=unknown")
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
        site = line_of("staging_cd_clock.py", '"Deployment admission timestamp is malformed")', -1)
        code, text, summary = self.run_main(clock.main, {"CD_STARTED_AT": PRIVATE})
        self.assertEqual(code, 30)
        self.assertEqual(legacy(text), "FAIL: deployment not admitted; invalid or expired admission time\n")
        self.assertTrue(text.startswith("FAIL: deployment not admitted; invalid or expired admission time"
                                        f" at=staging_cd_clock.py:{site} cause=transient\n"))
        self.assertIn("What changed: Nothing.", text)
        self.assertIn("| deploy-admission | admission-time:", summary)

    def test_invalid_component_result_names_its_check(self):
        import staging_cd_deploy_result as deploy_result
        code, text, _ = self.run_main(deploy_result.main, {"CD_COMPONENT_RESULT": PRIVATE})
        self.assertEqual(code, 1)
        self.assertEqual(legacy(text), "FAIL component-result reason=invalid-public-evidence\n")
        self.assertRegex(text.splitlines()[0], r" at=staging_cd_\w+\.py:\d+ cause=unknown\Z")

    def test_controller_input_error_and_unreadable_contract_are_explained(self):
        import staging_cd_invoke as invoke
        for outcome, expected, cause in ((40, "FAIL: deployment input or configuration error", "operator"),
                                         (invoke.CONTRACT_READ_FAILED, invoke.RESULTS[invoke.CONTRACT_READ_FAILED],
                                          "unknown")):
            with self.subTest(outcome=outcome), unittest.mock.patch.object(invoke, "deploy", return_value=outcome):
                code, text, summary = self.run_main(invoke.main, {})
            self.assertEqual(code, outcome)
            self.assertEqual(legacy(text).splitlines(), ["FAIL deploy", expected])
            self.assertIn(f"{expected} cause={cause}", text)
            self.assertIn("Next step: ", text)
            self.assertEqual(summary.splitlines()[-1], invoke.RESULTS[outcome])
        # A refused input without any public frame is still explained, without a check.
        output = io.StringIO()
        with unittest.mock.patch.dict("os.environ", {}, clear=True), contextlib.redirect_stdout(output):
            invoke.explain(40, None, None, "unreported", "unreported", "unreported", None, True)
        self.assertEqual(output.getvalue().splitlines()[0], "FAIL precondition=input cause=unknown")

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
        line = line_of("staging_cd_clock.py", "Deployment admission timestamp is malformed")
        with self.assertRaises(Refusal) as raised:
            elapsed(PRIVATE)
        self.assertIn(raised.exception.site, {f"staging_cd_clock.py:{line}", f"staging_cd_clock.py:{line - 1}"})

    def test_every_require_and_refusal_message_is_a_string_literal(self):
        # A literal message keeps each refusal a fixed, reviewable condition:
        # the reported module.py:LINE then identifies it without printing it.
        offenders, checked = literal_offenders(script_sources())
        self.assertGreater(checked, 400)
        self.assertEqual(offenders, [])

    def test_literal_check_covers_module_and_alias_call_forms(self):
        source = '''
import staging_cd_common as common
from staging_cd_common import require as check, Refusal as Refused
SUPERSEDED = "Fixed message"
def refuse(message):
    common.require(False, message)
    common.Refusal(message)
    check(False, message)
    Refused(message)
    common.require(False, SUPERSEDED)
    check(False, "Fixed message")
'''
        offenders, checked = literal_offenders({"fixture.py": source})
        self.assertEqual(checked, 6)
        self.assertEqual(offenders, ["fixture.py:6", "fixture.py:7", "fixture.py:8", "fixture.py:9"])


if __name__ == "__main__":
    unittest.main()
