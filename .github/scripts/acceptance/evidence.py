"""Reserve a safe output before networking and publish complete JSON atomically."""

import json
import os
from pathlib import Path
import stat
import uuid

from safety import Refusal


class Evidence:
    def __init__(self, destination):
        self.path = Path(destination).absolute()
        self.directory_fd = None
        self.reservation = None
        self.finished = False

    def __enter__(self):
        try:
            for component in (self.path.parent, *self.path.parent.parents):
                observed = component.lstat()
                if not stat.S_ISDIR(observed.st_mode):
                    raise Refusal("evidence_parent_not_real_directory")
                if (component / ".git").exists():
                    raise Refusal("evidence_inside_repository")
            parent = self.path.parent.lstat()
            if parent.st_uid != os.getuid() or stat.S_IMODE(parent.st_mode) != 0o700:
                raise Refusal("evidence_directory_not_private_owned_0700")
            self.directory_fd = os.open(self.path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            pinned = os.fstat(self.directory_fd)
            if (pinned.st_dev, pinned.st_ino) != (parent.st_dev, parent.st_ino):
                raise Refusal("evidence_directory_changed")
            fd = os.open(self.path.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                         0o600, dir_fd=self.directory_fd)
            with os.fdopen(fd, "w") as stream:
                observed = os.fstat(stream.fileno())
                self.reservation = (observed.st_dev, observed.st_ino)
                json.dump({"status": "reserved_incomplete"}, stream)
                stream.flush()
                os.fsync(stream.fileno())
            return self
        except (OSError, Refusal):
            if self.directory_fd is not None:
                os.close(self.directory_fd)
                self.directory_fd = None
            raise Refusal("evidence_destination_refused_before_requests") from None

    def publish(self, artifact):
        observed = os.stat(self.path.name, dir_fd=self.directory_fd, follow_symlinks=False)
        if (observed.st_dev, observed.st_ino) != self.reservation or not stat.S_ISREG(observed.st_mode):
            raise Refusal("evidence_reservation_changed")
        temporary = ".acceptance-" + uuid.uuid4().hex
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                     0o600, dir_fd=self.directory_fd)
        try:
            with os.fdopen(fd, "w") as stream:
                json.dump(artifact, stream, indent=2)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, self.path.name,
                       src_dir_fd=self.directory_fd, dst_dir_fd=self.directory_fd)
            os.fsync(self.directory_fd)
            self.finished = True
        finally:
            try:
                os.unlink(temporary, dir_fd=self.directory_fd)
            except FileNotFoundError:
                pass

    def __exit__(self, kind, value, traceback):
        try:
            if not self.finished:
                self.publish({"status": "incomplete", "reason": "phase_did_not_complete"})
        finally:
            os.close(self.directory_fd)
