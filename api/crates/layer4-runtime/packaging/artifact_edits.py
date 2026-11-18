"""Apply exact reviewed byte edits without broad content substitutions."""
import hashlib


def sha(data):
    return hashlib.sha256(data).hexdigest()


def transform(data, entry):
    if len(data) != entry["bytes"] or sha(data) != entry["source_sha256"]:
        raise ValueError("Artifact source differs")
    previous, parts = 0, []
    for edit in entry["edits"]:
        start, end = edit["start"], edit["end"]
        if (type(start) is not int or type(end) is not int
                or not previous <= start < end <= len(data)
                or sha(data[start:end]) != edit["sha256"]
                or not isinstance(edit["replacement"], str)):
            raise ValueError("Artifact edit differs")
        parts.extend((data[previous:start], edit["replacement"].encode()))
        previous = end
    parts.append(data[previous:])
    result = b"".join(parts)
    if sha(result) != entry["result_sha256"]:
        raise ValueError("Artifact result differs")
    return result
