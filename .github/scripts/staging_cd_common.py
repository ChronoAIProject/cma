"""Fixed refusal messages and input checks for public release automation."""
import re
import sys

from ci_outcome import source_site


class Refusal(ValueError):
    """Only fixed diagnostics may cross the public runner boundary.

    The message stays private. ``site`` names the public source line that
    refused, as ``module.py:LINE``, so a report can point at the exact check.
    """

    def __init__(self, *args):
        super().__init__(*args)
        self.site = raising_site(self)


def raising_site(error):
    # Skip this helper, the constructors of the refusal being built and the
    # shared require() helper: the caller of those is the check that refused.
    frame = sys._getframe(1)
    while frame is not None and (frame.f_code in HELPERS or frame.f_locals.get("self") is error):
        frame = frame.f_back
    return None if frame is None else source_site(frame.f_code.co_filename, frame.f_lineno)


def require(condition, message):
    if not condition:
        raise Refusal(message)


def matches(pattern, value):
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None


HELPERS = frozenset({raising_site.__code__, require.__code__})
