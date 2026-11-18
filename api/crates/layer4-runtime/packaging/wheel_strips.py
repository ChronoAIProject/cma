"""Rebuild a reviewed wheel with exact documentation edits and a valid RECORD."""
import base64
import csv
import io
import zipfile

from artifact_edits import sha, transform


def rebuild(data, entry):
    if sha(data) != entry["source_sha256"] or len(data) != entry["bytes"]:
        raise ValueError("Wheel source differs")
    edits = {item["path"]: item for item in entry["members"]}
    with zipfile.ZipFile(io.BytesIO(data)) as source:
        inventory = source.infolist()
        if len({item.filename for item in inventory}) != len(inventory):
            raise ValueError("Duplicate wheel member")
        if any(item.flag_bits & 1 for item in inventory):
            raise ValueError("Encrypted wheel member")
        records = [item.filename for item in inventory if item.filename.endswith(".dist-info/RECORD")]
        if len(records) != 1 or not set(edits).issubset(source.namelist()):
            raise ValueError("Invalid wheel inventory")
        record = records[0]
        payloads = {item.filename: source.read(item) for item in inventory}
        for name, edit in edits.items():
            payloads[name] = transform(payloads[name], edit)
        rows = []
        for item in inventory:
            name, payload = item.filename, payloads[item.filename]
            digest = base64.urlsafe_b64encode(bytes.fromhex(sha(payload))).rstrip(b"=").decode()
            rows.append((name, "" if name == record else "sha256=" + digest,
                         "" if name == record else str(len(payload))))
        text = io.StringIO(newline="")
        csv.writer(text, lineterminator="\n").writerows(rows)
        payloads[record] = text.getvalue().encode()
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as destination:
            for item in inventory:
                # Stored members make the outer digest independent of zlib versions.
                clean = zipfile.ZipInfo(item.filename, item.date_time)
                clean.external_attr = item.external_attr
                clean.create_system = item.create_system
                destination.writestr(clean, payloads[item.filename])
    result = output.getvalue()
    if entry.get("result_sha256") and sha(result) != entry["result_sha256"]:
        raise ValueError("Wheel result differs")
    return result
