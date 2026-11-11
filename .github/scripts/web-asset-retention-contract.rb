# frozen_string_literal: true

require "digest"
require "json"
require "yaml"

module WebAssetRetention
  ROOT = File.expand_path("../..", __dir__)
  BASELINE = File.join(ROOT, ".github/fixtures/web-retained-assets.json")
  MANIFEST = File.join(ROOT, "infra/web/deployment.yaml")

  def self.canonical(value)
    case value
    when Hash then value.sort.to_h.transform_values { |item| canonical(item) }
    when Array then value.map { |item| canonical(item) }
    else value
    end
  end

  def self.validate(deployment, baseline, live = nil)
    unless baseline["schema"] == 1 && deployment["kind"] == "Deployment" &&
           deployment.dig("metadata", "name") == baseline["deployment"] &&
           deployment.dig("metadata", "namespace") == baseline["namespace"]
      raise "Web retention resource identity mismatch"
    end
    items = deployment.dig("spec", "template", "spec", "initContainers")
    raise "Web retention initializer list missing" unless items.is_a?(Array) && !items.empty?
    names = items.map { |item| item.fetch("name") }
    unless names.uniq.length == names.length && names.all? { |name| name.start_with?("web-assets-") }
      raise "Web retention initializer names are invalid or duplicated"
    end
    unless items.all? { |item| item.fetch("image").match?(%r{@sha256:[0-9a-f]{64}\z}) }
      raise "Web retention images must be digest pinned"
    end
    unless items.any? { |item| item.fetch("image").end_with?("@" + baseline.fetch("rollback_image_digest")) }
      raise "Documented Web rollback assets are missing"
    end
    digest = Digest::SHA256.hexdigest(JSON.generate(canonical(items)))
    unless items.length == baseline["initializer_count"] && digest == baseline["init_containers_sha256"]
      raise "Web asset initializers differ from the reviewed live retention baseline"
    end
    if live
      unless live["read_only"] == true && live.dig("deployment", "name") == baseline["deployment"] &&
             live["initContainers"] == items
        raise "Web asset initializers differ from the fresh live projection"
      end
    end
    items.length
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    raise "Usage: web-asset-retention-contract.rb [live-projection.json]" unless ARGV.length <= 1
    count = WebAssetRetention.validate(YAML.load_file(WebAssetRetention::MANIFEST),
      JSON.parse(File.read(WebAssetRetention::BASELINE)), ARGV[0] && JSON.parse(File.read(ARGV[0])))
    puts "Web asset retention matches the reviewed baseline: #{count} initializers"
  rescue StandardError => error
    warn error.message
    exit 1
  end
end
