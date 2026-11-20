"""Derive a private member receipt from an independently authenticated artifact."""

import argparse
import json
import os
import sys
from pathlib import Path

from image_secrets.common import ScanError
from image_secrets.package_occurrences import prove_package_member


class Arguments(argparse.ArgumentParser):
    def error(self, _message):
        self.exit(2, "Invalid package proof arguments\n")


def main(argv=None):
    parser = Arguments(description=__doc__)
    parser.add_argument("--artifact", type=Path, required=True)
    parser.add_argument("--authenticated-sha256", required=True)
    parser.add_argument("--locator", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        receipt = {
            "schema": "cma-authenticated-package-member-v1",
            "artifact_sha256": args.authenticated_sha256,
            "member": prove_package_member(args.artifact, args.authenticated_sha256,
                                           args.locator, args.output.parent),
        }
        report = json.dumps(receipt, sort_keys=True).encode() + b"\n"
        handle = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(handle, "wb") as stream:
            stream.write(report)
    except (OSError, ScanError):
        print("Package member proof refused", file=sys.stderr)
        return 2
    print("Package member proof recorded")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
