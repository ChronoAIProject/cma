/** Apply expiring advisory exceptions without suppressing dependency audits. */

const levels = new Set(["info", "low", "moderate", "high", "critical"]);
const severe = new Set(["high", "critical"]);
const ghsa =
  /^GHSA-[23456789cfghjmpqrvwx]{4}-[23456789cfghjmpqrvwx]{4}-[23456789cfghjmpqrvwx]{4}$/;

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function object(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function productionPackages(tree) {
  const names = new Set();
  function visit(node) {
    requireValue(object(node), "Malformed production dependency tree");
    requireValue(
      !node.error && !node.missing && !node.invalid,
      "Production dependency tree is incomplete",
    );
    requireValue(
      node.problems === undefined ||
        (Array.isArray(node.problems) && !node.problems.length),
      "Production dependency tree contains npm problems",
    );
    if (node.name) names.add(node.name);
    if (node.dependencies === undefined) return;
    requireValue(
      object(node.dependencies),
      "Malformed production dependencies",
    );
    for (const [name, child] of Object.entries(node.dependencies)) {
      names.add(name);
      visit(child);
    }
  }
  visit(tree);
  return names;
}

function exceptionsById(entries, tree, today) {
  requireValue(Array.isArray(entries), "Audit exceptions must be an array");
  requireValue(/^\d{4}-\d{2}-\d{2}$/.test(today), "Invalid audit date");
  const production = productionPackages(tree);
  const result = new Map();
  for (const entry of entries) {
    requireValue(
      object(entry) &&
        Object.keys(entry).sort().join(",") ===
          "advisory,expires,package,reason,scope",
      "Invalid audit exception schema",
    );
    requireValue(
      typeof entry.advisory === "string" && ghsa.test(entry.advisory),
      "Invalid exception advisory",
    );
    requireValue(
      typeof entry.package === "string" &&
        /^(?:@[a-z0-9._-]+\/)?[a-z0-9._-]+$/.test(entry.package),
      "Invalid exception package",
    );
    requireValue(
      entry.scope === "dev-only",
      "Only dev-only exceptions are supported",
    );
    requireValue(
      typeof entry.reason === "string" && entry.reason.trim().length > 0,
      "Exception needs a reason",
    );
    const expires = new Date(`${entry.expires}T00:00:00Z`);
    requireValue(
      typeof entry.expires === "string" &&
        /^\d{4}-\d{2}-\d{2}$/.test(entry.expires) &&
        Number.isFinite(expires.valueOf()) &&
        expires.toISOString().slice(0, 10) === entry.expires,
      "Invalid exception expiry",
    );
    requireValue(
      today <= entry.expires,
      `Audit exception expired: ${entry.advisory} (${entry.expires})`,
    );
    requireValue(
      !production.has(entry.package),
      `Excepted package is in the production tree: ${entry.package}`,
    );
    requireValue(
      !result.has(entry.advisory),
      `Duplicate audit exception: ${entry.advisory}`,
    );
    result.set(entry.advisory, entry);
  }
  return result;
}

function advisoryId(advisory) {
  const match = /^https:\/\/github\.com\/advisories\/(GHSA-[a-z0-9-]+)$/.exec(
    advisory.url ?? "",
  );
  return match && ghsa.test(match[1]) ? match[1] : null;
}

export function evaluateAudit(
  report,
  tree,
  entries,
  today = new Date().toISOString().slice(0, 10),
) {
  const exceptions = exceptionsById(entries, tree, today);
  requireValue(
    object(report) &&
      report.auditReportVersion === 2 &&
      !report.error &&
      object(report.vulnerabilities),
    "Missing or unsupported npm audit report",
  );
  const vulnerabilities = report.vulnerabilities;
  const used = new Set();
  const blocking = new Set();
  for (const [name, vulnerability] of Object.entries(vulnerabilities)) {
    requireValue(
      object(vulnerability) &&
        vulnerability.name === name &&
        levels.has(vulnerability.severity) &&
        Array.isArray(vulnerability.via) &&
        vulnerability.via.length > 0,
      "Malformed audit vulnerability",
    );
    for (const via of vulnerability.via) {
      if (typeof via === "string") {
        requireValue(
          Object.hasOwn(vulnerabilities, via),
          `Unresolved audit dependency: ${via}`,
        );
        continue;
      }
      requireValue(
        object(via) &&
          levels.has(via.severity) &&
          typeof via.name === "string" &&
          typeof via.dependency === "string",
        "Malformed audit advisory",
      );
      if (!severe.has(via.severity)) continue;
      const id = advisoryId(via);
      const exception = exceptions.get(id);
      if (
        exception &&
        exception.package === name &&
        exception.package === via.name &&
        exception.package === via.dependency
      ) {
        used.add(id);
      } else {
        blocking.add(
          `${name}: ${id ?? "unidentified advisory"} (${via.severity})`,
        );
      }
    }
  }
  // npm repeats a root advisory through affected parent packages, sometimes
  // with cyclic peer edges. Resolve every severe parent to its actual sources
  // so excepting one root never hides an unrelated advisory on a parent.
  for (const [name, vulnerability] of Object.entries(vulnerabilities)) {
    if (!severe.has(vulnerability.severity)) continue;
    const pending = [name];
    const visited = new Set();
    let hasSevereSource = false;
    while (pending.length) {
      const current = pending.pop();
      if (visited.has(current)) continue;
      visited.add(current);
      for (const via of vulnerabilities[current].via) {
        if (typeof via === "string") pending.push(via);
        else if (severe.has(via.severity)) hasSevereSource = true;
      }
    }
    requireValue(
      hasSevereSource,
      `Cannot establish severe advisory sources for ${name}`,
    );
  }
  requireValue(
    blocking.size === 0,
    `Unexcepted high/critical advisories: ${[...blocking].sort().join("; ")}`,
  );
  for (const id of exceptions.keys())
    requireValue(used.has(id), `Unused audit exception: ${id}`);
  return [...used].sort().map((id) => exceptions.get(id));
}
