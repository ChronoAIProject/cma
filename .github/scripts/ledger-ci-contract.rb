# frozen_string_literal: true

# The dependency gates qualify source; these gates qualify the actual statically
# linked executable and its DELETE/EXTRA database behavior in every exact image.
module CIContract
  def self.ledger_images(inputs)
    ci = inputs.fetch("ci")
    gate = job(ci, "trigger-release-contract", "Trigger release contract")
    step(gate, "Ledger image qualification regressions",
      "node --test .github/scripts/runtime-ledger-evidence.test.mjs .github/scripts/runtime-ledger-downgrade.test.mjs")
    images = job(ci, "images", "Images")
    condition = "matrix.component == 'runtime' || matrix.component == 'runtime-desktop'"
    check = step(images, "Qualify exact runtime ledger storage",
      'node .github/scripts/runtime-ledger-evidence.mjs "cma-${IMAGE_COMPONENT}:ci" "$GITHUB_SHA" "$RUNNER_TEMP/ledger-storage.json"', condition)
    require!(check["env"] == { "IMAGE_COMPONENT" => "${{ matrix.component }}" }, "ledger evidence must use the exact matrix image")
    ledger_artifact(images, "always() && (#{condition})", "ledger-storage-${{ matrix.component }}")
    order(images, ["Load native image for scanning", "Qualify exact runtime ledger storage",
      "Retain ledger storage evidence", "Export tested native image"])
    runtime = job(inputs.fetch("publish"), "runtime-image-gate", "Runtime image evidence")
    check = step(runtime, "Qualify published runtime ledger storage",
      'node .github/scripts/runtime-ledger-evidence.mjs "$RUNTIME_REFERENCE" "$GITHUB_SHA" "$RUNNER_TEMP/ledger-storage.json"')
    require!(check["env"] == { "RUNTIME_REFERENCE" => "${{ steps.runtime.outputs.reference }}" }, "ledger evidence must use the verified immutable image")
    ledger_artifact(runtime, "always()", "release-ledger-storage")
    order(runtime, ["Pull verified immutable runtime", "Qualify published runtime ledger storage", "Retain ledger storage evidence"])
  end

  def self.ledger_artifact(row, condition, prefix)
    upload = step(row, "Retain ledger storage evidence", nil, condition)
    expected = prefix.start_with?("release-") ? "actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9" : "actions/upload-artifact@v4"
    require!(upload["uses"] == expected && upload["with"] == {
      "name" => "#{prefix}-${{ github.run_id }}-${{ github.run_attempt }}",
      "path" => "${{ runner.temp }}/ledger-storage.json",
      "if-no-files-found" => "error", "retention-days" => 90,
    }, "ledger image evidence must be exact, required and retained")
  end
end
