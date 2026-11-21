"""Production supersession is measured against main's newest release, never develop's head."""
import unittest
from unittest.mock import patch

from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_overtaken_deploy_tests import receipt
from staging_cd_versions import DEPLOYABLES
from staging_cd_watermark import SupersededVersions, require_released_versions

# Develop first parents, oldest first: every receipt's source, an ancestor of
# the release R, R itself, then develop commits a later release may merge.
OLDER, BETWEEN, RELEASED, AFTER, NEWEST = "a" * 40, "b" * 40, "2" * 40, "e" * 40, "f" * 40
CHAIN = [NEWEST, AFTER, RELEASED, BETWEEN, OLDER]
ALL = ["am", "eg", "runtime", "runtime-omarchy", "web"]


def history(newest=RELEASED):
    """Develop's first parents from the newest release's develop commit."""
    return DevelopHistory(newest, CHAIN[CHAIN.index(newest):])


def advertisement(*finals):
    """Each receipt's own final at OLDER plus `finals` as (component, version, source)."""
    rows = [(key, "0.10.1", OLDER) for key in ALL] + list(finals)

    def advertise(prefixes, *, runner, peeled=False):
        lines = []
        for index, (key, version, source) in enumerate(rows):
            ref = f"refs/tags/{DEPLOYABLES[key]}/v{version}"
            if any(ref.startswith(prefix) for prefix in prefixes):
                lines.append(f"{index + 1:040x}\t{ref}")
                if peeled:
                    lines.append(f"{source}\t{ref}^{{}}")
        return "\n".join(lines)
    return advertise


class ReleasedVersionsTests(unittest.TestCase):
    def check(self, *finals, keys=("web",), newest=RELEASED, version="0.10.1"):
        receipts = {key: receipt(key, version=version) for key in keys}
        with patch("staging_cd_watermark.advertise", side_effect=advertisement(*finals)):
            require_released_versions(receipts, history(newest))

    def test_only_a_higher_final_on_the_release_history_supersedes(self):
        # Develop publishes past every release; that final is a later release's to deploy.
        self.check(("web", "0.10.2", AFTER), ("eg", "0.10.2", NEWEST), keys=("eg", "web"))
        for source in (BETWEEN, RELEASED):
            with self.subTest(source=source), self.assertRaises(SupersededVersions) as raised:
                self.check(("web", "0.10.2", source), ("web", "0.10.3", AFTER), ("eg", "0.10.2", AFTER),
                           keys=("eg", "web"))
            self.assertEqual(raised.exception.components, {"web"})
        # The bound is the newest release's history, so it moves with each release.
        with self.assertRaises(SupersededVersions):
            self.check(("web", "0.10.2", AFTER), newest=NEWEST)

    def test_each_receipt_tag_must_still_be_advertised(self):
        with self.assertRaisesRegex(Refusal, "absent from component inventory"):
            self.check(version="0.10.2")


if __name__ == "__main__":
    unittest.main()
