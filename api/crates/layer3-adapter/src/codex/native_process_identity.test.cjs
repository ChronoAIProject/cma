const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { test } = require("node:test");
const { nativeProcessIdentity } = require("./native_process_identity.cjs");

const rosetta = "/run/rosetta/rosetta";
const binary = "/pinned/vendor/x86_64-unknown-linux-musl/bin/codex";
const content = Buffer.from("pinned native fixture bytes");
const expected = { path: binary, sha256: createHash("sha256").update(content).digest("hex") };
function fixture({ self = "/usr/local/bin/node", argv = [binary, "app-server"], bytes = content } = {}) {
  const reads = [];
  return { reads,
    readlinkSync(path) { assert.equal(path, "/proc/self/exe"); reads.push(path); return self; },
    readFileSync(path) {
      reads.push(path);
      if (path === "/proc/42/cmdline") return Buffer.from(argv.join("\0") + "\0");
      assert.equal(path, binary); return bytes;
    } };
}

test("native executable detection keeps the strict basename rule without reading argv or pins", () => {
  const io = fixture();
  assert.equal(nativeProcessIdentity(42, binary, {}, io), true);
  assert.deepEqual(io.reads, []);
  assert.equal(nativeProcessIdentity(42, "/usr/local/bin/node", expected, io), false);
  assert.deepEqual(io.reads, ["/proc/self/exe"]);
});

test("Rosetta child or self identity enables exact native argv and hash verification", () => {
  for (const [self, executable] of [["/usr/local/bin/node", rosetta], [rosetta, "/translated/guest"]]) {
    for (const argv of [[binary, "app-server"], [rosetta, binary, "app-server"]]) {
      const io = fixture({ self, argv });
      assert.equal(nativeProcessIdentity(42, executable, expected, io), true);
      assert(io.reads.includes(binary));
    }
  }
});

test("unrecognized translator paths cannot enable the argv fallback", () => {
  for (const executable of ["/usr/bin/qemu-x86_64", "/tmp/rosetta", rosetta + "-other"]) {
    const io = fixture({ self: executable });
    assert.equal(nativeProcessIdentity(42, executable, expected, io), false);
    assert(!io.reads.includes("/proc/42/cmdline"));
  }
});

test("Rosetta rejects launcher, wrong path, substring and non-app-server invocations", () => {
  for (const argv of [
    ["/usr/local/bin/node", binary, "app-server"],
    ["/other/codex", "app-server"], [binary + "-other", "app-server"],
    [binary, "--version"], [binary, "app-server", "extra"], [binary], [],
    ["/tmp/rosetta", binary, "app-server"],
  ]) {
    const io = fixture({ argv });
    assert.equal(nativeProcessIdentity(42, rosetta, expected, io), false);
    assert(!io.reads.includes(binary));
  }
});

test("Rosetta requires well-formed intended path and digest", () => {
  for (const pin of [{}, { ...expected, path: "relative/codex" },
    { ...expected, path: "/pinned/../codex" }, { ...expected, path: "/pinned/node" },
    { ...expected, sha256: "unknown" }, { path: binary }]) {
    assert.throws(() => nativeProcessIdentity(42, rosetta, pin, fixture()), /exact native executable/);
  }
});

test("Rosetta rejects changed native bytes even when argv and basename match", () => {
  assert.equal(nativeProcessIdentity(42, rosetta, expected,
    fixture({ bytes: Buffer.from("replacement") })), false);
});

test("Rosetta rejects missing terminators, empty and oversized cmdlines", () => {
  for (const bytes of [Buffer.alloc(0), Buffer.from(binary + "\0app-server"), Buffer.alloc(32769)]) {
    const io = fixture();
    io.readFileSync = () => bytes;
    assert.equal(nativeProcessIdentity(42, rosetta, expected, io), false);
  }
});

test("process and executable read failures propagate without manufacturing identity", () => {
  for (const code of ["ENOENT", "EACCES"]) {
    for (const target of ["self", "cmdline", "binary"]) {
      const io = fixture();
      const error = Object.assign(new Error("fixture read refused"), { code });
      if (target === "self") io.readlinkSync = () => { throw error; };
      else {
        const read = io.readFileSync;
        io.readFileSync = path => {
          if (path === (target === "binary" ? binary : "/proc/42/cmdline")) throw error;
          return read(path);
        };
      }
      assert.throws(() => nativeProcessIdentity(42,
        target === "self" ? "/usr/local/bin/node" : rosetta, expected, io), value => value === error);
    }
  }
});
