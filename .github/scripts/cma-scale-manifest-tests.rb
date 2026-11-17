# frozen_string_literal: true

require "minitest/autorun"
require_relative "cma-scale-manifest-contract"

class CmaScaleManifestTests < Minitest::Test
  def each_role
    CmaScaleManifests::ROLES.each_key do |role|
      yield role, *CmaScaleManifests.documents(role)
    end
  end

  def rejects
    each_role do |role, deployment, hpa, pdb|
      yield deployment, hpa, pdb
      assert_raises(RuntimeError, role) { CmaScaleManifests.validate(role, deployment, hpa, pdb) }
    end
  end

  def pod(deployment)
    deployment.fetch("spec").fetch("template").fetch("spec")
  end

  def test_reviewed_manifests_pass
    each_role { |role, *objects| assert CmaScaleManifests.validate(role, *objects) }
  end

  def test_rejects_single_replica_or_insufficient_ready_stabilization
    rejects { |d, _, _| d["spec"]["replicas"] = 1 }
    rejects { |d, _, _| d["spec"]["minReadySeconds"] = 0 }
  end

  def test_rejects_unavailable_rollout_and_short_grace
    rejects { |d, _, _| d["spec"]["strategy"]["rollingUpdate"]["maxUnavailable"] = 1 }
    rejects { |d, _, _| pod(d)["terminationGracePeriodSeconds"] = 30 }
  end

  def test_rejects_liveness_as_readiness_and_slow_removal
    rejects { |d, _, _| pod(d)["containers"][0]["readinessProbe"]["httpGet"]["path"] = "/livez" }
    rejects { |d, _, _| pod(d)["containers"][0]["readinessProbe"]["failureThreshold"] = 3 }
  end

  def test_rejects_missing_prestop_or_swallowed_am_drain_failure
    rejects { |d, _, _| pod(d)["containers"][0].delete("lifecycle") }
    d, h, p = CmaScaleManifests.documents("api")
    pod(d)["containers"][0]["lifecycle"]["preStop"]["exec"]["command"][-1] =
      "/usr/local/bin/cma --agent-manager-drain; exec /bin/sleep 30"
    assert_raises(RuntimeError) { CmaScaleManifests.validate("api", d, h, p) }
  end

  def test_rejects_eg_exec_hook_or_short_routing_propagation
    [
      { "exec" => { "command" => ["/bin/sh", "-c",
        "/usr/local/bin/cmaeg --replica-drain; status=$?; /bin/sleep 30; exit $status"] } },
      { "exec" => { "command" => ["/bin/sleep", "30"] } },
      { "sleep" => { "seconds" => 29 } },
      { "sleep" => { "seconds" => 30 }, "exec" => { "command" => ["/bin/sleep", "30"] } }
    ].each do |hook|
      d, h, p = CmaScaleManifests.documents("cmaeg")
      pod(d)["containers"][0]["lifecycle"]["preStop"] = hook
      assert_raises(RuntimeError) { CmaScaleManifests.validate("cmaeg", d, h, p) }
    end
  end

  def test_rejects_unbudgeted_requests_and_sidecar_cpu_denominator
    rejects { |d, _, _| pod(d)["containers"][0]["resources"]["requests"].delete("cpu") }
    rejects { |d, _, _| pod(d)["containers"] << { "name" => "unbudgeted-sidecar" } }
  end

  def test_rejects_unrelated_or_duplicate_topology_domains
    rejects { |d, _, _| pod(d)["topologySpreadConstraints"][0]["labelSelector"]["matchLabels"] = {} }
    rejects { |d, _, _| pod(d)["topologySpreadConstraints"] << pod(d)["topologySpreadConstraints"][0].dup }
    rejects { |d, _, _| pod(d)["topologySpreadConstraints"][0]["nodeTaintsPolicy"] = "Ignore" }
  end

  def test_rejects_spread_that_mixes_rollout_revisions
    [0, 1].each do |index|
      rejects { |d, _, _| pod(d)["topologySpreadConstraints"][index].delete("matchLabelKeys") }
      rejects { |d, _, _| pod(d)["topologySpreadConstraints"][index]["matchLabelKeys"] = ["app.kubernetes.io/name"] }
    end
  end

  def test_rejects_hard_zone_constraint_that_blocks_zone_loss_recovery
    rejects { |d, _, _| pod(d)["topologySpreadConstraints"][1]["whenUnsatisfiable"] = "DoNotSchedule" }
  end

  def test_rejects_pdb_that_permits_zero_available_at_one_replica
    rejects { |_, _, p| p["spec"] = { "maxUnavailable" => 1, "selector" => p["spec"]["selector"] } }
    rejects { |_, _, p| p["spec"]["selector"]["matchLabels"] = {} }
  end

  def test_rejects_pdb_that_can_block_node_drain_on_an_unhealthy_pod
    rejects { |_, _, p| p["spec"].delete("unhealthyPodEvictionPolicy") }
  end

  def test_rejects_wrong_hpa_target_and_unsupported_version
    rejects { |_, h, _| h["spec"]["scaleTargetRef"]["name"] = "another-workload" }
    rejects { |_, h, _| h["apiVersion"] = "autoscaling/v1" }
  end

  def test_rejects_unbudgeted_hpa_bounds_or_different_metric
    rejects { |_, h, _| h["spec"]["maxReplicas"] = 5 }
    rejects { |_, h, _| h["spec"]["maxReplicas"] = 20 }
    rejects { |_, h, _| h["spec"]["minReplicas"] = 1 }
    rejects { |_, h, _| h["spec"]["metrics"][0]["resource"]["name"] = "memory" }
  end

  def test_rejects_disabled_scale_up_and_aggressive_scale_down
    rejects { |_, h, _| h["spec"]["behavior"]["scaleUp"]["selectPolicy"] = "Disabled" }
    rejects { |_, h, _| h["spec"]["behavior"]["scaleDown"]["stabilizationWindowSeconds"] = 0 }
    rejects { |_, h, _| h["spec"]["behavior"]["scaleDown"]["policies"][0]["value"] = 2 }
  end

  def test_rejects_obsolete_or_unbounded_up_down_cadence
    rejects { |_, h, _| h["spec"]["behavior"]["scaleUp"]["policies"][0]["value"] = 1 }
    rejects { |_, h, _| h["spec"]["behavior"]["scaleUp"]["policies"].pop }
    rejects { |_, h, _| h["spec"]["behavior"]["scaleUp"]["policies"][1]["periodSeconds"] = 30 }
    rejects { |_, h, _| h["spec"]["behavior"]["scaleDown"]["stabilizationWindowSeconds"] = 600 }
    rejects { |_, h, _| h["spec"]["behavior"]["scaleDown"]["policies"][0]["periodSeconds"] = 60 }
  end

  def test_qualified_period_boundaries_and_fractional_round_up
    { 0.1 => 120, 90 => 120, 90.1 => 121, 120.2 => 151, 1770 => 1800 }.each do |p95, period|
      assert_equal period, CmaScaleManifests.downscale_period(p95)
    end
  end

  def test_missing_invalid_or_unrepresentable_timing_cannot_generate_hpa
    [nil, "90", 0, -1, Float::NAN, Float::INFINITY, 1770.1].each do |p95|
      assert_raises(RuntimeError) { CmaScaleManifests.hpa_candidate("api", p95) }
    end
  end

  def test_candidate_hpa_changes_only_the_measured_period
    each_role do |role, deployment, hpa, pdb|
      expected = Marshal.load(Marshal.dump(hpa))
      expected["spec"]["behavior"]["scaleDown"]["policies"][0]["periodSeconds"] = 181
      candidate = CmaScaleManifests.hpa_candidate(role, 150.2)
      assert_equal expected, candidate
      assert CmaScaleManifests.validate(role, deployment, candidate, pdb, down_period: 181)
      assert_raises(RuntimeError) { CmaScaleManifests.validate(role, deployment, candidate, pdb) }
      [119, 1801, 120.5].each do |period|
        assert_raises(RuntimeError) { CmaScaleManifests.validate(role, deployment, candidate, pdb, down_period: period) }
      end
      assert_equal hpa, CmaScaleManifests.hpa_candidate(role, 90)
    end
  end

  def test_patch_cannot_roll_back_images_configuration_or_metadata
    each_role do |role, _, _, _|
      patch = CmaScaleManifests.patch(role, 1)
      assert_equal ["spec"], patch.keys
      assert_equal 1, patch["spec"]["replicas"]
      assert_equal %w[replicas minReadySeconds strategy template].sort, patch["spec"].keys.sort
      assert_equal ["spec"], patch["spec"]["template"].keys
      assert_equal %w[terminationGracePeriodSeconds topologySpreadConstraints containers].sort, pod(patch).keys.sort
      assert_equal %w[name resources readinessProbe lifecycle].sort, pod(patch)["containers"][0].keys.sort
      assert_equal 0, CmaScaleManifests.patch(role, 0)["spec"]["replicas"]
      assert_equal 2, CmaScaleManifests.patch(role, 2)["spec"]["replicas"]
    end
  end

  def test_patch_refuses_unknown_role_and_unapproved_initial_scale
    assert_raises(KeyError) { CmaScaleManifests.patch("web", 2) }
    [-1, 3, 6, "2"].each { |replicas| assert_raises(RuntimeError) { CmaScaleManifests.patch("api", replicas) } }
  end
end
