#!/usr/bin/env node
const { request } = require('node:http');

const [operation, value] = process.argv.slice(2);
if (!['navigate', 'input'].includes(operation) || !value || !process.env.CMA_BROWSER_SOCKET) {
  process.stderr.write('Enabled Agent browser required. Usage: cma-browser navigate URL | input JSON\n');
  process.exit(2);
}
let body;
try { body = operation === 'navigate' ? { url: value } : { input: JSON.parse(value) }; }
catch { process.stderr.write('Browser input must be JSON.\n'); process.exit(2); }
const call = request({ socketPath: process.env.CMA_BROWSER_SOCKET, path: '/' + operation, method: 'POST',
  timeout: 15000, headers: { 'content-type': 'application/json' } }, response => {
  response.pipe(process.stdout);
  response.on('end', () => { process.exitCode = response.statusCode === 200 ? 0 : 1; });
});
call.on('error', () => { process.stderr.write('Agent browser is unavailable.\n'); process.exitCode = 1; });
call.on('timeout', () => call.destroy());
call.end(JSON.stringify(body));
