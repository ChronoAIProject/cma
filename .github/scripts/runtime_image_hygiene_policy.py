"""Strict runtime layer policy; one empty upstream file has a bound exception."""

from pathlib import PurePosixPath


ARCH_BASE = "sha256:e82623936fa1a69700b48e98b058df6eaab18b737c27473a7234a40fdecb9413"
# These are rootfs.diff_ids in the config referenced by the pinned manifest.
# Match the complete base prefix, never a caller-supplied label or base name.
ARCH_BASE_LAYERS = (
    "sha256:5f94078373e2a22c22cae07ef73c2c391af7593ad549e8a5059099e04d25c050",
    "sha256:8d36e27b25dabad06c0844ab91d2a20726a3605a8e5fb817804af5b50af08a5b",
)
PACMAN_RING = "etc/pacman.d/gnupg"
PACMAN_DIRECTORIES = {"etc", "etc/pacman.d", PACMAN_RING}
SANDBOX_PACMAN = "dev/shm/cma-pacman"
EMPTY_RING = "etc/pacman.d/gnupg/secring.gpg"
EMPTY_SHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
IPC_ID_DIR = "dev/shm/cma-machine-id"
IPC_ID_TARGET = "/" + IPC_ID_DIR + "/machine-id"
MACHINE_IDS = {"etc/machine-id", "var/lib/dbus/machine-id"}
CACHES = ("var/lib/pacman/sync", "var/cache/pacman/pkg")
NODE_MODULES = "usr/local/lib/node_modules/"
NPM_EXAMPLES = {"npm/node_modules/@npmcli/arborist/README.md", "npm/node_modules/ip-address/README.md"}
ZOD_SOURCE = "@agentclientprotocol/codex-acp/node_modules/zod/src/v4/"
ZOD_TEST_CONTENT = (ZOD_SOURCE + "classic/tests/", ZOD_SOURCE + "mini/tests/")


class HygieneRefusal(ValueError):
    """Only fixed, value-free reason codes cross the gate boundary."""


def require(value, reason):
    if not value:
        raise HygieneRefusal(reason)


def image_path(value):
    require(isinstance(value, str) and "\0" not in value, "unsafe_path")
    path = PurePosixPath(value)
    require(not path.is_absolute() and ".." not in path.parts, "unsafe_path")
    # Backslashes are ordinary POSIX filename bytes, never extraction syntax.
    return str(path)


def private_path(path):
    parts = PurePosixPath(path)
    return ("private-keys-v1.d" in parts.parts or parts.name == "secring"
            or parts.name.startswith("secring."))


def base_identity(component, diff_ids):
    if component == "runtime-omarchy" and tuple(diff_ids[:2]) == ARCH_BASE_LAYERS:
        return ARCH_BASE
    return None


def empty_ring_exception(base, layer, path, kind, size, digest):
    return (base == ARCH_BASE and layer == 0 and path == EMPTY_RING
            and kind == "file" and size == 0 and digest == EMPTY_SHA256)


def layer_entry(base, layer, path, kind, size, digest=None, target=None, component=None):
    require(kind in {"file", "directory", "symlink", "hardlink"}, "unsupported_entry")
    if path.startswith(NODE_MODULES):
        package_path = path[len(NODE_MODULES):]
        require(package_path not in NPM_EXAMPLES and not package_path.startswith(ZOD_TEST_CONTENT),
                "vendor_examples_layer")
    require(path != SANDBOX_PACMAN and not path.startswith(SANDBOX_PACMAN + "/"), "sandbox_keyring_layer")
    require(path != IPC_ID_DIR and not path.startswith(IPC_ID_DIR + "/"), "machine_identity_layer")
    if path in {"dev", "dev/shm"}:
        require(kind == "directory", "machine_identity_layer")
    if private_path(path) and kind != "directory":
        require(empty_ring_exception(base, layer, path, kind, size, digest), "private_keyring_layer")
        return True
    if path in MACHINE_IDS:
        require((kind == "file" and size == 0) or
                (kind == "symlink" and path == "var/lib/dbus/machine-id") or
                (component == "runtime" and path == "etc/machine-id"
                 and kind == "symlink" and target == IPC_ID_TARGET),
                "machine_identity_layer")
    if any(path == name or path.startswith(name + "/") for name in CACHES):
        require(kind == "directory" and path in CACHES, "package_cache_layer")
    return False


def apply_layer(state, entries, whiteouts, opaque):
    # OCI whiteouts hide lower-layer entries, regardless of member ordering.
    # They cannot erase a file introduced by this same layer.
    for target in whiteouts:
        for path in list(state):
            if path == target or path.startswith(target + "/"):
                del state[path]
    for parent in opaque:
        for path in list(state):
            if parent == "." or path.startswith(parent + "/"):
                del state[path]
    for path, value in entries.items():
        if value["kind"] != "directory":
            for child in list(state):
                if child.startswith(path + "/"):
                    del state[child]
        # Only policy-relevant paths need persistent overlay state. Every entry
        # was already checked; retaining the whole image makes updates quadratic.
        if (private_path(path) or path in MACHINE_IDS or path in PACMAN_DIRECTORIES
                or path.startswith(PACMAN_RING + "/")):
            state[path] = value
        else:
            state.pop(path, None)


def final_filesystem(state, component=None):
    require(not any(private_path(path) for path in state), "private_keyring_final")
    require(not any(path.startswith(PACMAN_RING + "/") for path in state)
            and all(state.get(path, {}).get("kind", "directory") == "directory" for path in PACMAN_DIRECTORIES),
            "initialized_pacman_keyring")
    identity = state.get("etc/machine-id", {})
    require((identity.get("kind") == "file" and identity.get("size") == 0) or
            (component == "runtime" and identity.get("kind") == "symlink" and identity.get("target") == IPC_ID_TARGET),
            "machine_identity_final")
    bus = state.get("var/lib/dbus/machine-id", {})
    require(bus.get("kind") == "symlink" and bus.get("target") in
            {"/etc/machine-id", "../../../etc/machine-id"}, "machine_identity_final")
