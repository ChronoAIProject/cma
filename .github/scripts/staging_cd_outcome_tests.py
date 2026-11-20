"""Every public failure code explains itself, and nothing private reaches the explanation."""
import contextlib
import io
import json
import re
import string
import subprocess
import tempfile
import unittest
from pathlib import Path

import ci_outcome
from ci_outcome import FAMILIES, codes, render, report

CATALOGUE_PATH = Path(__file__).with_name("ci_outcomes.json")
PRIVATE = "private-marker"


def vocabularies():
    """Map each catalogue family to the exact codes its producers can emit."""
    import staging_cd_allocate as allocate
    import staging_cd_build_cache as build_cache
    import staging_cd_component_admission as admission
    import staging_cd_deploy_result as deploy_result
    import staging_cd_finalize_diagnostics as finalize
    import staging_cd_invoke as invoke
    import staging_cd_publication_diagnostics as publication
    import staging_cd_refusal as refusal

    withheld = {reason for _, reason in build_cache.WITHHELD_REASONS}
    withheld |= {build_cache.withheld_reason(subprocess.SubprocessError()), build_cache.withheld_reason(OSError())}
    return {
        "deploy-private": set(refusal.REASONS) | {"unreported"},
        "deploy-verification": set(refusal.VERIFICATION_REASONS),
        "deploy-precondition": set(invoke.PRECONDITIONS),
        "deploy-component": {reason for reason in deploy_result.STATES.values() if reason != "none"},
        "publication": set(publication.REASONS),
        "publication-stage": set(publication.STAGES),
        "finalize": set(finalize.REASONS.values()) | finalize.FALLBACK_REASONS,
        "finalize-stage": set(finalize.STAGES),
        "allocation": set(allocate.REFUSAL_CODES.values()) | allocate.FALLBACK_CODES,
        "admission": set(admission.REASONS),
        "cache-withheld": withheld,
    }


def public_texts(outputs):
    return "\n".join(part for part in outputs if part is not None)


class CatalogueTests(unittest.TestCase):
    def test_every_code_of_every_vocabulary_has_an_entry(self):
        for family, expected in vocabularies().items():
            with self.subTest(family=family):
                missing = expected - codes(family).keys()
                self.assertFalse(missing, f"{family} codes lack a catalogue entry: {sorted(missing)}")

    def test_every_entry_is_complete_one_line_text_with_a_known_cause(self):
        entries = [("uncatalogued", "-", ci_outcome.CATALOGUE["uncatalogued"])]
        for family, spec in FAMILIES.items():
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
                    self.assertNotRegex(text, r"[\r\n{}|]|::")
                    self.assertLessEqual(len(text), 240)
        self.assertEqual(set(ci_outcome.CATALOGUE["causes"]), set(ci_outcome.CAUSES))

    def test_family_shapes_reference_only_known_families_and_placeholders(self):
        for family, spec in FAMILIES.items():
            with self.subTest(family=family):
                self.assertLessEqual(set(spec), {"line", "codes", "codes_from", "stages", "effect", "level", "title"})
                self.assertEqual(("codes" in spec) + ("codes_from" in spec), 1)
                if "codes_from" in spec:
                    self.assertIn("codes", FAMILIES[spec["codes_from"]])
                if "stages" in spec:
                    self.assertIn("codes", FAMILIES[spec["stages"]])
                self.assertIn(spec.get("level", "error"), ci_outcome.LEVELS)
                if "line" in spec:
                    fields = {name for _, name, _, _ in string.Formatter().parse(spec["line"]) if name is not None}
                    self.assertLessEqual(fields, set(ci_outcome.PLACEHOLDERS))
                    self.assertNotIn("\n", spec["line"])
                for key in ("effect", "title"):
                    if key in spec:
                        self.assertNotRegex(spec[key], r"[\r\n{}|]|::")
                for code in codes(family):
                    self.assertRegex(code, r"\A[A-Za-z][A-Za-z0-9_-]{0,47}\Z")
        self.assertEqual(ci_outcome.COMPONENTS, __import__("staging_cd_targets").TARGETS)

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
        line, annotation, row = render("publication", "validation_refused", subject="publication", stage="build",
                                       site="staging_cd_build.py:58", component="am")
        self.assertEqual(line, "FAIL publication stage=build reason=validation_refused component=am"
                               " at=staging_cd_build.py:58 cause=change")
        self.assertTrue(annotation.startswith("::error file=.github/scripts/staging_cd_build.py,line=58,"
                                              "title=publication%3A Input check refused::"))
        self.assertIn("Next step: Open the reported line", annotation)
        self.assertIn("At this stage: Build the same Dockerfile", annotation)
        self.assertEqual(row.count(" | "), 4)
        line, _, _ = render("allocation", "VERSION_LABEL_REQUIRED", subject="allocation")
        self.assertEqual(line, "FAIL allocation VERSION_LABEL_REQUIRED cause=change")
        line, annotation, _ = render("deploy-verification", "pod-restarts", subject="deploy",
                                     component=["web", "am"], recovery="unreported")
        self.assertEqual(line, "FAIL verification=pod-restarts recovery=unreported component=am,web cause=change")
        self.assertIn("What changed: Staging may have changed", annotation)
        self.assertIn("title=deploy%3A Deployment unverified%3A Pods restarted", annotation)
        line, annotation, _ = render("deploy-release", "pod-restarts", subject="deploy", component="web")
        self.assertEqual(line, "FAIL release=pod-restarts recovery=verified component=web cause=change")
        self.assertIn("restored and verified the previous release", annotation)

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
        for family, code in (("publication", PRIVATE), (PRIVATE, "build_failed"), (None, None)):
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
    def test_report_prints_line_and_annotation_and_appends_one_table(self):
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
            text = summary.read_text()
            self.assertEqual(text.count(ci_outcome.HEADER), 1)
            self.assertTrue(text.startswith("Infrastructure revision: fixture\n\n" + ci_outcome.HEADER))
            with summary.open("a") as stream:
                stream.write("Admission-to-result seconds: 1.00\n")
            report("allocation", "READ_FAILED", subject="allocation", stream=output, env=env)
            self.assertEqual(summary.read_text().count(ci_outcome.HEADER), 2)

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

    def test_error_site_names_public_frames_and_never_the_message(self):
        def fail():
            raise KeyError(PRIVATE)
        try:
            fail()
        except KeyError as error:
            site = ci_outcome.error_site(error)
        self.assertRegex(site, r"\Astaging_cd_outcome_tests\.py:\d+\Z")
        namespace = {}
        exec(compile("def outside():\n    raise ValueError('x')\n", "/elsewhere/" + PRIVATE + ".py", "exec"), namespace)
        try:
            namespace["outside"]()
        except ValueError as error:
            self.assertRegex(ci_outcome.error_site(error), r"\Astaging_cd_outcome_tests\.py:\d+\Z")
            error.__traceback__ = error.__traceback__.tb_next
            self.assertIsNone(ci_outcome.error_site(error))
        error = ValueError()
        error.site = "../" + PRIVATE + ".py:1"
        self.assertIsNone(ci_outcome.error_site(error))


if __name__ == "__main__":
    unittest.main()
