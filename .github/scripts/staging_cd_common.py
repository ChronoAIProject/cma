"""Fixed refusal messages and input checks for public release automation."""
import re


class Refusal(ValueError):
    """Only fixed diagnostics may cross the public runner boundary."""


def require(condition, message):
    if not condition:
        raise Refusal(message)


def matches(pattern, value):
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None
