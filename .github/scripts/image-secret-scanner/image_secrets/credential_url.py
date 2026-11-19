"""Recognize credential-bearing URI and IRI authorities embedded in arbitrary bytes."""

import ipaddress
import re

# Unreserved characters and sub-delimiters. Percent escapes are validated
# separately so a malformed escape cannot be accepted as a shorter host.
_NAME = rb"A-Za-z0-9._~!$&'()*+,;=%\x80-\xff-"
_HIGH_BYTE = re.compile(rb"[\x80-\xff]")
# URL consumers percent-encode printable userinfo punctuation during parsing.
# Authority delimiters and non-printable bytes end the candidate.
_USERINFO = re.compile(rb"[^\x00-\x20\x7f/:@?#]*:[^\x00-\x20\x7f/@?#]+@")
_TO_NEXT_AT = re.compile(rb"[^\x00-\x20\x7f/@?#]*@")
_SPECIAL = re.compile(rb"(?:^|[^A-Za-z0-9+.\-])(?:https?|wss?|ftp|file)$", re.I)
_REG_NAME = re.compile(rb"[" + _NAME + rb"]+")
_BAD_ESCAPE = re.compile(rb"%(?![0-9A-Fa-f]{2})")
_IPV6 = re.compile(rb"\[([0-9A-Fa-f:.]{2,45})(?:%25[A-Za-z0-9._~%-]+)?\]")
_IPVFUTURE = re.compile(rb"\[[vV][0-9a-fA-F]+\.[A-Za-z0-9._~!$&'()*+,;=:-]+\]")


def _printable_end(data, start, end):
    # Binary bytes may surround a legitimate internationalized authority.
    # A malformed UTF-8 sequence or non-printable scalar ends the current run.
    high = _HIGH_BYTE.search(data, start, end)
    while high is not None:
        pos = high.start()
        first = data[pos]
        width = 2 if first < 0xE0 else 3 if first < 0xF0 else 4
        try:
            scalar = data[pos:min(pos + width, end)].decode("utf-8")
        except UnicodeDecodeError:
            return pos
        if not scalar.isprintable():
            return pos
        high = _HIGH_BYTE.search(data, pos + width, end)
    return end


def _valid_host(data, start):
    if data[start:start + 1] != b"[":
        name = _REG_NAME.match(data, start)
        if name is None:
            return False
        end = _printable_end(data, start, name.end())
        return end > start and _BAD_ESCAPE.search(data, start, end) is None
    literal = _IPV6.match(data, start)
    if literal is None:
        return _IPVFUTURE.match(data, start) is not None
    if _BAD_ESCAPE.search(data, start, literal.end()):
        return False
    try:
        ipaddress.IPv6Address(literal.group(1).decode("ascii"))
    except ipaddress.AddressValueError:
        return False
    return True


def _authority(data, separator):
    start = separator + 3
    userinfo = _USERINFO.match(data, start)
    while userinfo is not None:
        end = userinfo.end()
        if _printable_end(data, start, end) < end:
            return False
        # Web-special schemes treat a backslash as the start of a path.
        if data.find(b"\\", start, end) >= 0 and _SPECIAL.search(data, max(0, separator - 8), separator):
            return False
        if _valid_host(data, end):
            return True
        # A second @ can be the actual host delimiter; parsers encode earlier
        # ones into the user information.
        start = end
        userinfo = _TO_NEXT_AT.match(data, start)
    return False


def search(data):
    """Return whether the input contains a credential-bearing URL."""
    # Start at literal separators instead of attempting the scheme regex at
    # every offset in long printable strings. A scheme suffix is sufficient
    # for detection inside surrounding text and binary data.
    end = data.find(b"://")
    while end >= 0:
        i = end - 1
        while i >= 0 and data[i] in b"0123456789+.-":
            i -= 1
        if i >= 0 and data[i:i + 1].isalpha():
            if _authority(data, end):
                return True
        end = data.find(b"://", end + 3)
    return False
