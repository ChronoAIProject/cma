import { readFile } from "node:fs/promises";
import { plan } from "./matrix.mjs";

// Optional input is a sanitized binding packet, never a bearer/config file.
const file = process.argv[2];
const binding = file ? JSON.parse(await readFile(file, "utf8")) : {};
const result = plan(binding);
process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
if (result.counts.blocked) process.exitCode = 2;
