"""Read the EG base-schema requirement from the candidate source tree."""

import os
from pathlib import Path
import re
import sys

LABEL = "ai.chrono.cmaeg.schema-version"
SOURCE = "api/crates/cmaeg/src/records/mongo_schema.rs"


def required_version(root):
    source = (Path(root) / SOURCE).read_text()
    declarations = re.findall(r"^pub const BASE_SCHEMA_VERSION: i32 = (.*);$", source, re.M)
    if len(declarations) != 1 or not re.fullmatch(r"[1-9][0-9]{0,8}", declarations[0]):
        raise ValueError("EG base schema constant is missing or ambiguous")
    return int(declarations[0])


def labels(component, root):
    return {LABEL: str(required_version(root))} if component in {"eg", "cmaeg"} else {}


def main():
    try:
        if len(sys.argv) != 2:
            raise ValueError("Expected one component")
        value = labels(sys.argv[1], Path.cwd())
        rendered = next((key + "=" + version for key, version in value.items()), "")
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write("label=" + rendered + "\n")
        return 0
    except (OSError, ValueError, KeyError):
        print("FAIL schema label")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
