import assert from "node:assert/strict";
import { test } from "node:test";
import { missingOperations } from "./check-api-docs.mjs";

const document = { paths: { "/api/v1/agents/{id}": { get: {}, delete: {} } } };
const inventory = (rows) =>
  `<!-- openapi-operations:start -->\n${rows}\n<!-- openapi-operations:end -->`;

test("checks methods individually, not only a shared path", () => {
  assert.deepEqual(
    missingOperations(
      document,
      inventory("| `GET /api/v1/agents/{id}` | body |"),
    ),
    ["DELETE /api/v1/agents/{id}"],
  );
});

test("accepts complete route rows while ignoring path metadata", () => {
  assert.deepEqual(
    missingOperations(
      {
        paths: { ...document.paths, "/other": { summary: "Not an operation" } },
      },
      inventory(
        "| `GET /api/v1/agents/{id}` | body |\n| `DELETE /api/v1/agents/{id}` | empty |",
      ),
    ),
    [],
  );
});

test("does not count examples, another resource or prose outside the inventory", () => {
  const markdown =
    "| `GET /api/v1/agents/{id}` | body |\n" +
    inventory("| `GET /api/v1/agents/{other}` | body |");
  assert.deepEqual(missingOperations(document, markdown), [
    "GET /api/v1/agents/{id}",
    "DELETE /api/v1/agents/{id}",
  ]);
});

test("fails closed on missing inventory or an unusable OpenAPI export", () => {
  assert.throws(
    () => missingOperations(document, "No inventory"),
    /no operation inventory/,
  );
  for (const value of [
    null,
    {},
    { paths: [] },
    { paths: {} },
    { paths: { invalid: {} } },
  ]) {
    assert.throws(() => missingOperations(value, inventory("")));
  }
});
