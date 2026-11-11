import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";

export function missingConfiguration(sources, markdown) {
  const keys = new Set(
    sources.flatMap((source) =>
      [...source.matchAll(/"((?:CHRONO|CMAEG)_[A-Z0-9_]+)"/g)].map((match) => match[1]),
    ),
  );
  if (keys.size === 0)
    throw new Error("No quoted configuration keys were found.");
  const documented = new Set(
    [...markdown.matchAll(/`((?:CHRONO|CMAEG)_[A-Z0-9_]+)`/g)].map((match) => match[1]),
  );
  return [...keys].sort().filter((key) => !documented.has(key));
}

function sourcesIn(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    if (entry.isSymbolicLink())
      throw new Error("Configuration source symlinks are not supported.");
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sourcesIn(path);
    return entry.isFile() && entry.name.endsWith(".rs")
      ? [readFileSync(path, "utf8")]
      : [];
  });
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    const root = fileURLToPath(new URL("../../", import.meta.url));
    for (const [crate, docs] of [
      ["cma", ["CMA_Operations", "CMA_Admission_Append", "CMA_Reporting_Pool", "CMA_Retained_Recovery", "CMA_Bounded_Responses", "CMA_Log_Segments_Operations"]],
      ["cmaeg", ["CMAEG_Operations"]],
    ]) {
      const sources = [
        readFileSync(join(root, `api/crates/${crate}/src/config.rs`), "utf8"),
        ...sourcesIn(join(root, `api/crates/${crate}/src/config`)),
      ];
      const markdown = docs
        .map((doc) => readFileSync(join(root, `docs/${doc}.md`), "utf8"))
        .join("\n");
      const missing = missingConfiguration(sources, markdown);
      if (missing.length > 0)
        throw new Error(`Undocumented configuration keys for ${crate}:\n${missing.join("\n")}`);
    }
    console.log(
      "Operations documentation covers every quoted deployment configuration key.",
    );
  } catch (error) {
    console.error(
      error instanceof Error
        ? error.message
        : "Configuration documentation check failed.",
    );
    process.exitCode = 1;
  }
}
