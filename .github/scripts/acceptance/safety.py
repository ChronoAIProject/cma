"""Private storage and bounded HTTPS for dedicated production acceptance."""

import datetime
from contextlib import contextmanager
import json
import os
from pathlib import Path
import stat
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

SECRET_ROOT = Path.home() / ".config" / "cma-accept"
NYX_ORIGIN = "https://nyx-api.chrono-ai.fun"
CMA_ORIGIN = "https://cma.nyxid.ai"
ALLOWED_ORIGINS = {
    NYX_ORIGIN, CMA_ORIGIN, "https://bot.chrono-ai.fun",
    "https://bot-gateway.chrono-ai.fun",
    "https://api.nyxid.ai",
    "https://nyxid.ai",
}


class Refusal(Exception):
    """A fixed diagnostic safe to include in an aggregate report."""


def outside_quiet_window(now=None):
    now = now or datetime.datetime.now(datetime.timezone.utc)
    if now.astimezone(datetime.timezone(datetime.timedelta(hours=8))).hour == 2:
        raise Refusal("quiet_window_0200_0259_sgt")


def private_root():
    root = SECRET_ROOT.absolute()
    for parent in (root, *root.parents):
        if parent.is_symlink():
            raise Refusal("secret_directory_symlink")
        if (parent / ".git").exists():
            raise Refusal("secret_directory_inside_repository")
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = root.stat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise Refusal("secret_directory_permissions")
    return root


def private_path(name):
    if not name or Path(name).name != name or name in (".", ".."):
        raise Refusal("private_name_not_leaf")
    return private_root() / name


def read_private(name, default=None):
    path = private_path(name)
    if path.is_symlink():
        raise Refusal("secret_file_symlink")
    if not path.exists():
        return default
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd) as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
            raise Refusal("secret_file_permissions")
        return json.load(stream)


def write_private(name, value):
    path = private_path(name)
    if path.is_symlink():
        raise Refusal("secret_file_symlink")
    if path.exists():
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
            raise Refusal("secret_file_permissions")
    temporary = path.with_name(".pending-" + uuid.uuid4().hex)
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        directory_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary.exists():
            temporary.unlink()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Http:
    def __init__(self):
        self.opener = urllib.request.build_opener(NoRedirect())
        self.last_request = 0.0
        self.count = 0
        self.writes_authorized = False
        self.cleaning = False

    @contextmanager
    def cleanup_budget(self):
        previous = self.cleaning
        self.cleaning = True
        try:
            yield
        finally:
            self.cleaning = previous

    def reserve_cleanup_time(self, now=None):
        now = now or datetime.datetime.now(datetime.timezone.utc)
        local = now.astimezone(datetime.timezone(datetime.timedelta(hours=8)))
        if not self.cleaning and local.hour == 1 and local.minute >= 30:
            raise Refusal("cleanup_time_reserved_before_quiet_hour")

    def request(self, origin, path, method="GET", token=None, body=None,
                headers=None, limit=1024 * 1024, form=None):
        if origin not in ALLOWED_ORIGINS or not path.startswith("/") or path.startswith("//"):
            raise Refusal("unapproved_destination")
        if method not in ("GET", "HEAD", "OPTIONS") and not self.writes_authorized:
            raise Refusal("production_writes_require_verified_actors")
        if urllib.parse.urlsplit(path).netloc or "\r" in path or "\n" in path:
            raise Refusal("invalid_path")
        outside_quiet_window()
        self.reserve_cleanup_time()
        if self.count >= (500 if self.cleaning else 350):
            raise Refusal("request_budget_exhausted")
        time.sleep(max(0, 1.05 - (time.monotonic() - self.last_request)))
        outside_quiet_window()
        fields = {"Accept": "application/json", "User-Agent": "cma-acceptance/1"}
        fields.update(headers or {})
        if token:
            fields["Authorization"] = "Bearer " + token
        data = None
        if body is not None and form is not None:
            raise Refusal("ambiguous_request_body")
        if body is not None:
            data = json.dumps(body).encode()
            if len(data) > 65536:
                raise Refusal("request_body_limit")
            fields["Content-Type"] = "application/json"
        if form is not None:
            data = urllib.parse.urlencode(form, doseq=True).encode()
            if len(data) > 65536:
                raise Refusal("request_body_limit")
            fields["Content-Type"] = "application/x-www-form-urlencoded"
        request = urllib.request.Request(origin + path, data=data, headers=fields, method=method)
        self.last_request = time.monotonic()
        self.count += 1
        started = time.monotonic()
        try:
            response = self.opener.open(request, timeout=10)
        except urllib.error.HTTPError as response_error:
            response = response_error
        except (urllib.error.URLError, TimeoutError):
            raise Refusal("request_transport_failure") from None
        with response:
            payload = response.read(limit + 1)
            if len(payload) > limit:
                raise Refusal("response_body_limit")
            return response.code, dict(response.headers), payload, round(time.monotonic() - started, 3)


def decode(payload):
    try:
        return json.loads(payload)
    except (ValueError, UnicodeDecodeError):
        raise Refusal("invalid_json_response") from None
