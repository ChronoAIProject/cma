/**
 * No class may be declared in more than one stylesheet.
 *
 * Every sheet under `src/` is imported globally — there are no CSS modules —
 * so a class declared in two files silently merges and load order decides the
 * winner. An audit found thirteen names in that state. Four changed how a
 * shared component rendered, and one of them was why a picker clipped its own
 * menu: an unrelated selection list called itself `.picker` and handed every
 * `<Picker>` in the product an `overflow-y` it never asked for.
 *
 * The audit was a snapshot. This is what stops it happening again.
 *
 * Deliberate sharing is allowed and named. An entry here is a claim that two
 * sheets are meant to touch one class, and that somebody decided so.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

const ROOT = 'src';

/**
 * Classes two sheets are meant to share, and why.
 *
 * Named rather than pattern-matched: a list that grows by accident is the same
 * as no check at all.
 */
const SHARED = new Map([
  [
    'button',
    'components.css gives every interactive control one transition; ui/Button.css owns the rest',
  ],
  ['pill', 'as .button — the shared transition rule'],
  ['input', 'as .button — the shared transition rule'],
]);

/** Every .css file under src/. */
function sheets(dir) {
  const found = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      found.push(...sheets(path));
    } else if (entry.endsWith('.css')) {
      found.push(path);
    }
  }
  return found;
}

/**
 * The class names a sheet declares rules for.
 *
 * Preludes only — the text before each `{` — so a class named inside a
 * declaration value is never counted. Nested blocks (`@media`, `@supports`)
 * are walked the same way, since a rule inside one still declares the class.
 */
function declaredIn(css) {
  const names = new Set();
  // Strip comments first: a selector inside one is not a declaration.
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, '');
  for (const match of stripped.matchAll(/(^|[}{])([^{}]*)\{/g)) {
    const prelude = match[2] ?? '';
    if (prelude.trimStart().startsWith('@')) {
      continue;
    }
    for (const found of prelude.matchAll(/\.(-?[_a-zA-Z][\w-]*)/g)) {
      const name = found[1];
      if (name !== undefined) {
        names.add(name);
      }
    }
  }
  return names;
}

function main() {
  const byClass = new Map();
  for (const sheet of sheets(ROOT)) {
    for (const name of declaredIn(readFileSync(sheet, 'utf8'))) {
      const files = byClass.get(name) ?? new Set();
      files.add(sheet);
      byClass.set(name, files);
    }
  }

  const problems = [];
  let allowed = 0;
  for (const [name, files] of [...byClass].sort()) {
    if (files.size < 2) {
      continue;
    }
    if (SHARED.has(name)) {
      allowed += 1;
      continue;
    }
    problems.push(`.${name} is declared in ${[...files].sort().join(' and ')}`);
  }

  /*
   * An allowlist entry for a class that no longer collides is stale. Left in,
   * it would silently permit a future collision on that name — which is the
   * failure this check exists to prevent, arriving through the exception.
   */
  for (const [name] of SHARED) {
    if ((byClass.get(name)?.size ?? 0) < 2) {
      problems.push(`.${name} is allowed to be shared but no longer is — remove the exception`);
    }
  }

  if (problems.length > 0) {
    console.error(`css collisions: ${String(problems.length)} problem(s)`);
    for (const problem of problems) {
      console.error(`  ${problem}`);
    }
    process.exitCode = 1;
    return;
  }
  console.log(
    `css collisions: none across ${String(byClass.size)} classes ` +
      `(${String(allowed)} shared deliberately)`,
  );
}

main();
