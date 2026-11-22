"""Conservatively keep AM/runtime contract transitions in one admitted set."""
import os
from pathlib import Path
import shlex
import subprocess

from staging_cd_common import require

GROUP = frozenset({"am", "runtime", "runtime-omarchy"})
VERSION_FILE = "api/crates/layer3-adapter/runtime-versions.json"
RECIPE = "api/crates/layer4-runtime/Dockerfile"
LABELS = frozenset({"dev.chrono.cma.runtime-contract", "dev.chrono.cma.runtime-bootstrap",
                    "dev.chrono.cma.home-layout"})


def runtime_labels(text):
    labels = {key: set() for key in LABELS}
    for line in text.replace("\\\n", " ").splitlines():
        if not line.strip() or line.split(maxsplit=1)[0].upper() != "LABEL":
            continue
        words = shlex.split(line, comments=True)
        for word in words[1:]:
            key, separator, value = word.partition("=")
            require(separator, "Runtime contract label syntax is unmodeled")
            if key in labels:
                require(value.isascii() and value.isdecimal(), "Runtime contract label is unmodeled")
                labels[key].add(value)
    require(all(len(values) == 1 for values in labels.values()), "Runtime contract labels are ambiguous")
    return tuple((key, next(iter(values))) for key, values in sorted(labels.items()))


class ContractPolicy:
    def __init__(self, history, root=None, runner=subprocess.run):
        self.history, self.root, self.runner = history, root or Path.cwd(), runner
        self.signatures = {}
        self.transitions = {}

    def git(self, *args):
        env = {key: value for key, value in os.environ.items() if key in ("PATH", "LANG", "LC_ALL")}
        env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_NO_REPLACE_OBJECTS="1", GIT_TERMINAL_PROMPT="0", GIT_NO_LAZY_FETCH="1")
        result = self.runner(["git", *args], cwd=self.root, env=env, capture_output=True,
                             text=True, timeout=30, check=False)
        require(result.returncode == 0 and len(result.stdout) <= 2 * 1024 * 1024,
                "Runtime contract source is unavailable")
        return result.stdout

    def signature(self, source):
        self.history.position(source)
        if source not in self.signatures:
            from staging_cd_contract import declared_versions
            versions = declared_versions(self.git("show", source + ":" + VERSION_FILE))
            self.signatures[source] = (tuple(sorted(versions.items())),
                                       runtime_labels(self.git("show", source + ":" + RECIPE)))
        return self.signatures[source]

    def transition(self, source):
        self.history.position(source)
        if source not in self.transitions:
            commit = self.git("cat-file", "commit", source).split("\n\n", 1)[0]
            parents = [line[7:] for line in commit.splitlines() if line.startswith("parent ")]
            require(bool(parents), "Runtime contract comparison boundary is unavailable")
            current = self.signature(source)
            # Introducing the declaration is a full group transition. Historical
            # sources without it cannot provide compatibility authority.
            parent_has_versions = self.git("ls-tree", "--name-only", parents[0], "--", VERSION_FILE).strip()
            self.transitions[source] = (not parent_has_versions
                                        or self.signature(parents[0]) != current)
        return self.transitions[source]

    def transition_sources(self, tasks):
        sources = {task["receipt" if task["mode"] == "resume" else "reservation"]["source"]
                   for component in GROUP & tasks.keys() for task in tasks[component]}
        return [source for source in sorted(sources) if self.transition(source)]

    def required_sources(self, tasks):
        transitions = self.transition_sources(tasks)
        if not transitions:
            return {}
        # A retry omits already-finalized peers from its publication matrix.
        # Resume only the exact transition source already authorized by this
        # matrix, never an arbitrary newer tag or an invented successful task.
        source = max(transitions, key=self.history.position)
        return {component: source for component in GROUP - tasks.keys()}

    def group_reason(self, tasks, receipts):
        members = GROUP & tasks.keys()
        if not members:
            return None
        # Do not short-circuit: every source must be readable and on first-parent
        # history even after one transition has already been detected.
        if self.transition_sources(tasks) and not GROUP <= receipts.keys():
            return "contract-group-incomplete"
        chosen = {self.signature(receipt["source"]) for key, receipt in receipts.items() if key in GROUP}
        if len(chosen) > 1:
            return "contract-group-mismatch"
        return None
