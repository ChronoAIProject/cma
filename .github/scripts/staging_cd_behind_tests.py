"""Allocation plans exactly the lanes behind develop, never a current one."""
import unittest

from staging_cd_behind_fixture import Develop
from staging_cd_compatibility import GROUP


class PlanBehindTests(unittest.TestCase):
    def test_publisher_only_merge_plans_no_current_lane_but_retries_a_failed_one(self):
        develop = Develop()
        first = develop.push({"web", "eg"})
        develop.finalize(first["web"])  # The EG build fails.
        develop.complete(801)
        # The shared selector selects no image for a publisher repair.
        repaired = develop.push(set(), verified={"web"})
        self.assertEqual(repaired.keys(), {"eg"})
        retry = repaired["eg"]["reservation"]
        self.assertEqual((repaired["eg"]["mode"], retry["source"], retry["previous"]),
                         ("publish", develop.source(0), first["eg"]["reservation"]["version"]))
        develop.finalize(repaired["eg"])
        develop.complete(802)
        self.assertEqual(develop.push(set(), verified={"web", "eg"}), {})

    def test_failed_attempt_at_a_no_longer_selected_source_is_retried_there(self):
        # The runtime-omarchy shape: a publisher-wide merge last attempted the
        # lane. The narrower selector must not strand it below that attempt.
        develop = Develop()
        develop.push({"runtime-omarchy"})
        develop.complete(801)
        wide = develop.push({"runtime-omarchy"})
        develop.complete(802)
        develop.unselect(1)
        retried = develop.push({"web"})
        self.assertEqual(retried.keys(), {"runtime-omarchy", "web"})
        task = retried["runtime-omarchy"]
        self.assertEqual((task["mode"], task["reservation"]["source"], task["reservation"]["previous"]),
                         ("publish", develop.source(1), wide["runtime-omarchy"]["reservation"]["version"]))

    def test_current_lane_with_a_final_at_a_no_longer_selected_source_stays_current(self):
        develop = Develop()
        develop.finalize(develop.push({"web"})["web"])
        develop.complete(801)
        wide = develop.finalize(develop.push({"web"})["web"])
        develop.complete(802)
        develop.unselect(1)
        gateway = develop.push({"eg"}, verified={"web"})
        self.assertEqual(gateway.keys(), {"eg"})
        develop.finalize(gateway["eg"])
        develop.complete(803)
        # Without a controller proof it resumes only its own newest final.
        self.assertEqual(develop.push(set(), verified={"eg"}),
                         {"web": {"component": "web", "mode": "resume", "receipt": wide}})

    def test_group_peer_final_at_a_no_longer_selected_source_keeps_the_transition_whole(self):
        develop = Develop()
        anchor = develop.source(0)
        for task in develop.push(GROUP, transition=anchor).values():
            develop.finalize(task)
        develop.complete(801)
        wide = {key: develop.finalize(task) for key, task in develop.push(GROUP, transition=anchor).items()}
        develop.complete(802)
        develop.unselect(1)
        successor = develop.push({"am"}, transition=anchor)
        self.assertEqual(successor.keys(), GROUP)
        self.assertEqual((successor["am"]["mode"], successor["am"]["reservation"]["source"]),
                         ("publish", develop.source(2)))
        for key in GROUP - {"am"}:
            self.assertEqual(successor[key], {"component": key, "mode": "resume", "receipt": wide[key]})


if __name__ == "__main__":
    unittest.main()
