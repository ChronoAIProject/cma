#!/usr/bin/env node
const { request } = require('node:http');

const [operation, url] = process.argv.slice(2);
if (operation !== 'navigate' || !url || !process.env.CMA_BROWSER_SOCKET) {
  process.stderr.write('Browser navigation requires an enabled Agent browser. Usage: cma-browser navigate URL\n');
  process.exit(2);
}
const call = request({ socketPath: process.env.CMA_BROWSER_SOCKET, path: '/navigate', method: 'POST',
  timeout: 15000, headers: { 'content-type': 'application/json' } }, response => {
  response.pipe(process.stdout);
  response.on('end', () => { process.exitCode = response.statusCode === 200 ? 0 : 1; });
});
call.on('error', () => { process.stderr.write('Agent browser is unavailable.\n'); process.exitCode = 1; });
call.on('timeout', () => call.destroy());
call.end(JSON.stringify({ url }));
