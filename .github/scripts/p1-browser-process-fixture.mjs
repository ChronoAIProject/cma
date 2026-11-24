import { writeFile } from "node:fs/promises";
import { join } from "node:path";

export async function createProcessFixture(directory) {
  const child = join(directory, "child.cjs");
  const parent = join(directory, "parent.cjs");
  await writeFile(
    child,
    `const { createServer } = require('node:http');
const { writeFileSync } = require('node:fs');
const marker = process.argv[2];
const server = createServer();
server.listen(0, '127.0.0.1', () => {
  writeFileSync(marker, JSON.stringify({ port: server.address().port }));
});
process.on('SIGTERM', () => {});`,
  );
  await writeFile(
    parent,
    `const { spawn } = require('node:child_process');
const { existsSync } = require('node:fs');
const [child, marker, mode] = process.argv.slice(2);
spawn(process.execPath, [child, marker], { stdio: 'inherit' });
setInterval(() => {
  if (!existsSync(marker)) return;
  if (mode === 'success') process.exit(0);
  if (mode === 'failure') process.exit(3);
}, 20);`,
  );
  return { child, parent };
}
