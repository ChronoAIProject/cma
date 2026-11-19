"""Recognize credential-bearing RFC 3986 authorities embedded in arbitrary bytes."""

import ipaddress
import re

# Unreserved characters and sub-delimiters. Percent escapes are validated
# separately so a malformed escape cannot be accepted as a shorter host.
_NAME = rb"A-Za-z0-9._~!$&'()*+,;=%-"
_USERINFO = re.compile(rb"[" + _NAME + rb"]*:[:" + _NAME + rb"]+@")
_REG_NAME = re.compile(rb"[" + _NAME + rb"]+")
_BAD_ESCAPE = re.compile(rb"%(?![0-9A-Fa-f]{2})")
_IPV6 = re.compile(rb"\[([0-9A-Fa-f:.]{2,45})(?:%25[A-Za-z0-9._~%-]+)?\]")
_IPVFUTURE = re.compile(rb"\[[vV][0-9a-fA-F]+\.[A-Za-z0-9._~!$&'()*+,;=:-]+\]")


def _valid_host(data, start):
    if data[start:start + 1] != b"[":
        name = _REG_NAME.match(data, start)
        return name is not None and _BAD_ESCAPE.search(data, start, name.end()) is None
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
            start = end + 3
            userinfo = _USERINFO.match(data, start)
            if (userinfo is not None and _BAD_ESCAPE.search(data, start, userinfo.end()) is None
                    and _valid_host(data, userinfo.end())):
                return True
        end = data.find(b"://", end + 3)
    return False
