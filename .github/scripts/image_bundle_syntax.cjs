// Parse image bytes as data; never import, evaluate or execute the bundle.
'use strict';
const fs = require('node:fs');
const crypto = require('node:crypto');
const parser = require('./image-content-vendor/babel-parser-7.29.3.cjs');

function member(node, depth = 0) {
  if (!node || depth > 32) return false;
  if (node.type === 'Identifier') return true;
  return node.type === 'MemberExpression' && !node.computed && !node.optional
    && node.property.type === 'Identifier' && member(node.object, depth + 1);
}

function comparison(node, depth = 0) {
  if (!node || depth > 64) return false;
  if (node.type === 'BinaryExpression') {
    return node.operator === '===' && member(node.left) && member(node.right);
  }
  return node.type === 'LogicalExpression' && node.operator === '&&'
    && comparison(node.left, depth + 1) && comparison(node.right, depth + 1);
}

function reviewedShape(node) {
  // The reviewed capture starts inside === and ends at the terminal call's
  // identifier. Its complete context is comparisons followed by a two-member call.
  const tail = node.right;
  return node.type === 'LogicalExpression' && node.operator === '&&' && comparison(node.left)
    && tail.type === 'CallExpression' && !tail.optional && tail.callee.type === 'Identifier'
    && tail.arguments.length === 2 && tail.arguments.every(arg => arg.type === 'MemberExpression' && member(arg));
}

function prove({text, findings}) {
  const ast = parser.parse(text, {sourceType: 'unambiguous', errorRecovery: false, tokens: true});
  const contexts = [], operators = new Map();
  for (const token of ast.tokens) {
    if (token.value === '===' && token.type.label === '==/!=/===/!==') operators.set(token.start + 1, token);
  }
  const stack = [ast];
  while (stack.length) {
    const node = stack.pop();
    if (!node || typeof node !== 'object') continue;
    if (node.type === 'LogicalExpression' && reviewedShape(node)) contexts.push(node);
    for (const [key, value] of Object.entries(node)) {
      if (['loc', 'comments', 'tokens', 'errors', 'extra'].includes(key)) continue;
      if (Array.isArray(value)) stack.push(...value);
      else if (value && typeof value === 'object') stack.push(value);
    }
  }
  function valid(start, end) {
    const operator = operators.get(start);
    if (!operator || operator.end !== start + 2) return false;
    return contexts.some(node => node.start <= start && node.right.callee.start < end
      && node.right.callee.end === end && operator.end <= node.right.callee.start);
  }
  // Match the pinned detector's ASCII capture and RE2 whitespace grammar.
  // Both regex indices and Babel offsets use UTF-16, including astral characters.
  const capture = /(?:api[_-]?key|access[_-]?token|auth[_-]?token|client[_-]?secret|password|passwd|secret|credential|private[_-]?key)["']?[\t\n\f\r ]*[:=][\t\n\f\r ]*["']?([A-Za-z0-9_./+!@#$%^&*=-]{12,})/gid;
  const requested = new Set(findings.map(f => f.candidate_sha256)), candidates = new Map();
  let cursor = 0, line = 1;
  for (const match of text.matchAll(capture)) {
    while (cursor < match.index) if (text[cursor++] === '\n') line++;
    const hash = crypto.createHash('sha256').update(match[1]).digest('hex');
    if (!requested.has(hash)) continue;
    const [start, end] = match.indices[1];
    const item = candidates.get(hash) || {value: match[1], valid: true, lines: new Set()};
    item.valid &&= valid(start, end);
    item.lines.add(line);
    candidates.set(hash, item);
  }
  // Duplicate captures may collapse into a detector row. Every occurrence must
  // have the reviewed context, including occurrences outside a detector match.
  for (const item of candidates.values()) {
    let start = text.indexOf(item.value);
    while (start !== -1) {
      item.valid &&= valid(start, start + item.value.length);
      start = text.indexOf(item.value, start + 1);
    }
  }
  return findings.filter(f => {
    const item = candidates.get(f.candidate_sha256);
    return item && item.valid && item.lines.has(f.line);
  }).map(f => f.finding_id);
}

try {
  const input = fs.readFileSync(0);
  if (input.length > 32 * 1024 * 1024) throw new Error('input-limit');
  process.stdout.write(JSON.stringify(prove(JSON.parse(input.toString('utf8')))));
} catch {
  // Parser errors can quote image-controlled content. Emit no diagnostics.
  process.stdout.write('[]');
  process.exitCode = 1;
}
