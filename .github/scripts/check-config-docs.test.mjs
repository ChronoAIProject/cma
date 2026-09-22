import assert from "node:assert/strict";
import { test } from "node:test";
import { missingConfiguration } from "./check-config-docs.mjs";

test("CMAEG keys require exact documentation across nested source modules", () => {
  assert.deepEqual(missingConfiguration([
    'read("CMAEG_LISTEN"); read("CMAEG_METRICS_LISTEN")',
    'read("CMAEG_ACK_TEXT"); error("CMAEG_DIAGNOSTIC is invalid")',
  ], "`CMAEG_LISTEN` `CMAEG_ACK_TEXT`"), ["CMAEG_METRICS_LISTEN"]);
});

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
