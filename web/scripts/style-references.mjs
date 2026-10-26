import postcss from 'postcss';

const references = (value) => [...value.matchAll(/var\(\s*(--[\w-]+)/g)].map((match) => match[1]);

/**
 * CMA's theme tokens resolve on body. React Flow's stylesheet instead defines
 * local defaults and optional overrides with var() fallbacks inside its canvas.
 * Identify that API from the pinned dependency, never from a name prefix. Only
 * uses confined to Flow selectors move to the rendered-canvas check; a use
 * elsewhere, an unknown name, or a CMA alias still has the body contract.
 */
export function styleReferences(css, flowCss) {
  const flowNames = new Set(references(flowCss));
  const body = new Set();
  const flow = new Set();
  const sheet = postcss.parse(css);
  sheet.walkAtRules((rule) => {
    for (const name of references(rule.params)) body.add(name);
  });
  sheet.walkDecls((declaration) => {
    const rule = declaration.parent;
    const inFlow =
      rule.type === 'rule' &&
      rule.selectors.every(
        (selector) =>
          /^(?:\.delivery-flow\s+)?\.react-flow(?:__[\w-]+)?(?=[\s.#[:>]|$)/.test(selector) &&
          !/[+~]/.test(selector),
      );
    for (const name of references(declaration.value)) {
      (inFlow && flowNames.has(name) ? flow : body).add(name);
    }
  });
  // Keep the original complete inventory, including any non-declaration use.
  for (const name of references(css)) {
    if (!flow.has(name)) body.add(name);
  }
  for (const name of body) flow.delete(name);
  return { body: [...body].sort(), flow: [...flow].sort() };
}
