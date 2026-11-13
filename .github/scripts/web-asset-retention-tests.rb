# frozen_string_literal: true

require "minitest/autorun"
require_relative "web-asset-retention-contract"

class WebAssetRetentionTests < Minitest::Test
  def setup
    @deployment = YAML.load_file(WebAssetRetention::MANIFEST)
    @baseline = JSON.parse(File.read(WebAssetRetention::BASELINE))
    @items = @deployment.fetch("spec").fetch("template").fetch("spec").fetch("initContainers")
  end

  def validate(live = nil)
    WebAssetRetention.validate(@deployment, @baseline, live)
  end

  def test_shipped_manifest_matches_recorded_production_inventory
    assert_equal @baseline["initializer_count"], validate
  end

  def test_dropping_one_historical_release_is_refused
    @items.delete_at(1)
    assert_raises(RuntimeError) { validate }
  end

  def test_missing_documented_rollback_is_refused_even_with_updated_fingerprint
    @items.reject! { |item| item["image"].end_with?("@" + @baseline["rollback_image_digest"]) }
    @baseline["initializer_count"] = @items.length
    @baseline["init_containers_sha256"] = Digest::SHA256.hexdigest(JSON.generate(WebAssetRetention.canonical(@items)))
    assert_raises(RuntimeError) { validate }
  end

  def test_each_effective_initializer_field_is_covered
    %w[image command args resources volumeMounts securityContext imagePullPolicy terminationMessagePath terminationMessagePolicy].each do |key|
      original = @items[0][key]
      @items[0][key] = "changed"
      assert_raises(RuntimeError, key) { validate }
      @items[0][key] = original
    end
  end

  def test_order_and_duplicate_names_are_covered
    @items[0], @items[1] = @items[1], @items[0]
    assert_raises(RuntimeError) { validate }
    @items[1]["name"] = @items[0]["name"]
    assert_raises(RuntimeError) { validate }
  end

  def test_yaml_mapping_order_does_not_cause_false_drift
    @items[0] = @items[0].to_a.reverse.to_h
    assert_equal @baseline["initializer_count"], validate
  end

  def test_fresh_live_list_changes_require_new_capture_and_review
    live = {"read_only" => true, "deployment" => {"name" => "cma-web"}, "initContainers" => Marshal.load(Marshal.dump(@items))}
    assert_equal @baseline["initializer_count"], validate(live)
    live["initContainers"].pop
    assert_raises(RuntimeError) { validate(live) }
  end

  def test_wrong_resource_or_untrusted_projection_is_refused
    @deployment["metadata"]["namespace"] = "other"
    assert_raises(RuntimeError) { validate }
    @deployment["metadata"]["namespace"] = @baseline["namespace"]
    assert_raises(RuntimeError) { validate({"read_only" => false}) }
  end
end
