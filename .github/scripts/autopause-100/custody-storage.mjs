import {constants} from 'node:fs';
import * as filesystem from 'node:fs/promises';
import {dirname, join, resolve} from 'node:path';
import {check, hash} from '../scale-tests/lib/checks.mjs';

export const defaultCustodyIO = filesystem;
export const JOURNAL_FILE = 'custody.jsonl';

export function privateStat(stat, directory = false) {
  check((directory ? stat.isDirectory() : stat.isFile() && stat.nlink === 1)
    && stat.uid === process.getuid() && !(stat.mode & 0o077), 'custody_private_path_required');
}

export async function openDirectory(path, io) {
  check(await io.realpath(path) === path, 'custody_private_path_required');
  const handle = await io.open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_DIRECTORY);
  try {privateStat(await handle.stat(), true); return handle;}
  catch (error) {await handle.close(); throw error;}
}

export async function openPrivateFile(path, flags, io) {
  // Nonblocking open lets type validation refuse a FIFO without hanging before stat.
  const handle = await io.open(path, flags | constants.O_NOFOLLOW | constants.O_NONBLOCK, 0o600);
  try {privateStat(await handle.stat()); return handle;}
  catch (error) {await handle.close(); throw error;}
}

export async function readBounded(handle, limit, observeChanges = false) {
  const before = await handle.stat(); privateStat(before);
  check(before.size <= limit, 'custody_file_bound');
  const chunks = []; let offset = 0;
  while (offset < before.size) {
    const buffer = Buffer.alloc(Math.min(64 * 1024, before.size - offset));
    const {bytesRead} = await handle.read(buffer, 0, buffer.length, offset);
    if (bytesRead === 0 && observeChanges) break;
    check(bytesRead > 0, 'custody_file_changed');
    offset += bytesRead; chunks.push(buffer.subarray(0, bytesRead));
  }
  const after = await handle.stat();
  const changed = !sameFile(before, after) || offset !== before.size;
  check(observeChanges || !changed, 'custody_file_changed');
  return {bytes: Buffer.concat(chunks), stat: after, changed};
}

export function sameFile(before, after) {
  return before.dev === after.dev && before.ino === after.ino && before.size === after.size
    && before.mtimeMs === after.mtimeMs && before.ctimeMs === after.ctimeMs;
}

export class CustodyStorage {
  #directory; #folder; #file; #io; #length = 0;
  constructor(directory, folder, file, io) {
    this.#directory = directory; this.#folder = folder; this.#file = file; this.#io = io;
  }
  static async create(directory, io = defaultCustodyIO) {
    const path = resolve(directory); let parent = await openDirectory(dirname(path), io);
    let folder; let file;
    try {
      // The directory is the permanent exclusive-creator fence. A failed or
      // crashed creator never grants a later process permission to overwrite it.
      await io.mkdir(path, {mode: 0o700}); await parent.sync();
      folder = await openDirectory(path, io);
      file = await openPrivateFile(join(path, JOURNAL_FILE), constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL, io);
      await folder.sync();
      await parent.close(); parent = null;
      return new CustodyStorage(path, folder, file, io);
    } catch (error) {
      try {if (file) await file.close();} finally {if (folder) await folder.close();}
      throw error;
    } finally {if (parent) await parent.close();}
  }
  async #verifyDirectory() {
    const current = await this.#io.lstat(this.#directory);
    const opened = await this.#folder.stat();
    privateStat(current, true);
    check(current.dev === opened.dev && current.ino === opened.ino, 'custody_directory_changed');
    const file = await this.#file.stat();
    const linked = await this.#io.lstat(join(this.#directory, JOURNAL_FILE));
    privateStat(linked);
    check(file.dev === linked.dev && file.ino === linked.ino, 'custody_file_changed');
  }
  async append(line) {
    await this.#verifyDirectory();
    check((await this.#file.stat()).size === this.#length, 'custody_file_changed');
    await this.#file.writeFile(line);
    await this.#file.sync();
    await this.#folder.sync();
    await this.#verifyDirectory();
    const length = this.#length + Buffer.byteLength(line);
    check((await this.#file.stat()).size === length, 'custody_file_changed');
    this.#length = length;
  }
  async content(name, bytes) {
    await this.#verifyDirectory();
    const file = await openPrivateFile(join(this.#directory, name),
      constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL, this.#io);
    try {await file.writeFile(bytes); await file.sync();}
    finally {await file.close();}
    await this.#folder.sync();
    await this.#verifyDirectory();
  }
  async verify(refs) {
    await this.#verifyDirectory();
    for (const ref of refs) await verifyContent(this.#directory, ref, this.#io);
  }
  async close() {
    try {await this.#file.close();} finally {await this.#folder.close();}
  }
}

export async function verifyContent(directory, ref, io) {
  const path = join(directory, ref.name);
  const file = await openPrivateFile(path, constants.O_RDONLY, io);
  try {
    const {bytes, stat} = await readBounded(file, ref.bytes);
    check(bytes.length === ref.bytes && hash(bytes) === ref.sha256, 'custody_content_digest_invalid');
    if (ref.role === 'key') check(bytes.every(byte => byte >= 0x21 && byte <= 0x7e), 'custody_key_invalid');
    check(sameFile(stat, await io.lstat(path)), 'custody_file_changed');
  } finally {await file.close();}
}
