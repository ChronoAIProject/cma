"""Build a Web prepare archive from an incumbent and a selected candidate."""

import argparse
from pathlib import Path

from staging_cd_common import Refusal
from staging_cd_web_overlay import WebArchive, assemble_prepare, encoded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    release_fields = ("ARCHIVE", "DIGEST", "SOURCE", "VERSION")
    parser.add_argument("--incumbent", nargs=4, metavar=release_fields, required=True)
    parser.add_argument("--candidate", nargs=4, metavar=release_fields, required=True)
    parser.add_argument("--output", type=Path, required=True, help="Existing directory for the prepare archive")
    parser.add_argument("--max-files", type=int, required=True)
    parser.add_argument("--max-bytes", type=int, required=True)
    args = parser.parse_args()
    if not args.output.is_dir():
        parser.error("--output must name an existing directory")
    incumbent = WebArchive(Path(args.incumbent[0]), *args.incumbent[1:])
    candidate = WebArchive(Path(args.candidate[0]), *args.candidate[1:])
    try:
        result = assemble_prepare(incumbent, candidate, args.output, max_files=args.max_files, max_bytes=args.max_bytes)
    except Refusal as error:
        parser.exit(1, f"{error}\n")
    except (OSError, ValueError):
        parser.exit(1, "Web prepare archive could not be read or written\n")
    result["prepare"]["archive"] = str(result["prepare"]["archive"])
    print(encoded(result).decode())


if __name__ == "__main__":
    main()
