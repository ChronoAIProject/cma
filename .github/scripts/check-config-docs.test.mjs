import assert from "node:assert/strict";
import { test } from "node:test";
import { missingConfiguration } from "./check-config-docs.mjs";

test("requires documentation for every complete key literal across modules", () => {
  assert.deepEqual(
    missingConfiguration(
      ['read("CHRONO_FIRST")', 'read("CHRONO_SECOND")'],
      "`CHRONO_FIRST` is configured.",
    ),
    ["CHRONO_SECOND"],
  );
});

test("deduplicates keys and excludes diagnostic prose", () => {
  assert.deepEqual(
    missingConfiguration(
      [
        'read("CHRONO_FIRST"); error("CHRONO_SECOND must be present"); read("CHRONO_FIRST")',
      ],
      "`CHRONO_FIRST` is configured.",
    ),
    [],
  );
});

test("requires exact documented names and rejects empty source sets", () => {
  assert.deepEqual(
    missingConfiguration(['"CHRONO_FIRST"'], "`CHRONO_FIRST_EXTRA`"),
    ["CHRONO_FIRST"],
  );
  assert.throws(
    () => missingConfiguration([], ""),
    /No quoted configuration keys/,
  );
});
