"""Generated MIME signatures require complete bounded records and exact inputs."""

import copy
import struct
import unittest

from image_secrets.key_vectors import sha
from image_secrets.mime_markers import cache_records, magic_records, mime_matches


class MimeMarkerTests(unittest.TestCase):
    def fixture(self, form):
        marker = b"-----BEGIN " + b"PRIVATE KEY-----\n"
        name = "application/pkcs8"
        if form == "magic":
            prefix = b"MIME-Magic\0\n[50:" + name.encode() + b"]\n>0=" + len(marker).to_bytes(2, "big")
            data = prefix + marker + b"\n"
            records = [
                {
                    "offset": len(prefix),
                    "value_length": len(marker),
                    "mime_type": name,
                    "marker_only_value": True,
                    "record_newline_after_value": True,
                }
            ]
        else:
            data = bytearray(176)
            struct.pack_into(">HH", data, 0, 1, 2)
            struct.pack_into(">II", data, 24, 80, 172)
            data[44 : 44 + len(name)] = name.encode()
            struct.pack_into(">III", data, 80, 1, len(marker) + 1, 92)
            struct.pack_into(">IIII", data, 92, 50, 44, 1, 108)
            struct.pack_into(">IIIIIIII", data, 108, 0, 1, 1, len(marker), 140, 0, 0, 0)
            data[140 : 140 + len(marker)] = marker
            # The next section begins after the four-byte-aligned value.
            struct.pack_into(">I", data, 28, (140 + len(marker) + 3) & ~3)
            data = bytes(data)
            records = [
                {
                    "offset": 140,
                    "matchlet_offset": 108,
                    "value_length": len(marker),
                    "mime_type": name,
                    "start_offset": 0,
                    "range_length": 1,
                    "word_size": 1,
                    "mask_offset": 0,
                    "children": 0,
                    "marker_only_value": True,
                }
            ]
        state = {
            "usr/share/mime/packages/synthetic.xml": b"synthetic signatures",
            "usr/bin/generator": b"tool",
        }
        structure = {
            "format": form,
            "records": records,
            "inputs": [
                {
                    "path": "usr/share/mime/packages/synthetic.xml",
                    "sha256": sha(b"synthetic signatures"),
                    "bytes": 20,
                }
            ],
            "generator": {"path": "usr/bin/generator", "sha256": sha(b"tool")},
        }
        if form == "cache":
            structure.update(format_version=[1, 2], magic_entries=1)
        return data, state, structure

    def test_exact_records_and_all_inputs_pass_without_claiming_reproduction(self):
        for form in ("magic", "cache"):
            data, state, structure = self.fixture(form)
            self.assertTrue(mime_matches(data, state, structure))

    def test_missing_changed_extra_inputs_and_generator_refuse(self):
        for form in ("magic", "cache"):
            data, state, structure = self.fixture(form)
            for case in ("input", "generator", "missing", "extra"):
                changed = dict(state)
                if case == "input":
                    changed["usr/share/mime/packages/synthetic.xml"] += b"x"
                if case == "generator":
                    changed["usr/bin/generator"] += b"x"
                if case == "missing":
                    changed.pop("usr/share/mime/packages/synthetic.xml")
                if case == "extra":
                    changed["usr/share/mime/packages/extra.xml"] = b"unreviewed"
                self.assertFalse(mime_matches(data, changed, structure))

    def test_magic_length_type_and_unconsumed_tail_refuse(self):
        data, state, structure = self.fixture("magic")
        at = structure["records"][0]["offset"]
        for changed in (
            data + b"tail",
            data[:-1],
            data[: at - 1] + b"\xff" + data[at:],
            data.replace(b"application/pkcs8", b"application/other"),
            data[:at] + b"body" + data[at:],
            data.replace(b">0=", b">9="),
        ):
            self.assertFalse(mime_matches(changed, state, structure))
        with self.assertRaises(ValueError):
            magic_records(b"unrecognized")

    def test_cache_pointers_lengths_types_children_and_padding_refuse(self):
        data, state, structure = self.fixture("cache")
        for offset, value in (
            (24, 9999),
            (80, 2),
            (96, 9999),
            (104, 109),
            (120, 9999),
            (124, 141),
            (128, 1),
            (132, 1),
            (136, 108),
            (108, 2),
        ):
            changed = bytearray(data)
            struct.pack_into(">I", changed, offset, value)
            self.assertFalse(mime_matches(bytes(changed), state, structure))
        changed = bytearray(data)
        changed[44] = ord("x")
        self.assertFalse(mime_matches(bytes(changed), state, structure))
        changed = bytearray(data)
        changed[167] = 1
        self.assertFalse(mime_matches(bytes(changed), state, structure))
        wrong = copy.deepcopy(structure)
        wrong["records"][0]["offset"] += 1
        self.assertFalse(mime_matches(data, state, wrong))
        with self.assertRaises(ValueError):
            cache_records(data[:40], structure)


if __name__ == "__main__":
    unittest.main()
