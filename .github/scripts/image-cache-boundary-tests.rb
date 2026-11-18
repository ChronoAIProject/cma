# frozen_string_literal: true
require 'minitest/autorun'
require 'yaml'
require 'tmpdir'
require 'open3'
require 'securerandom'
require_relative 'image-cache-boundary'

class ImageCacheBoundaryTest < Minitest::Test
  def setup
    @documents = ImageCacheBoundary::JOBS.to_h do |name, _id|
      [name, YAML.load_file(File.join(__dir__, '../workflows', name + '.yml'))]
    end
  end

  def test_current_workflows_keep_build_outputs_local
    assert ImageCacheBoundary.validate(@documents)
    ImageCacheBoundary::JOBS.each do |workflow, id|
      builds = @documents[workflow]['jobs'][id]['steps'].select { |s| s.fetch('uses', '').start_with?('docker/build-push-action@') }
      assert builds.all? { |s| s.fetch('with').key?('cache-from') }, 'cache reads must survive'
    end
  end

  def test_each_build_rejects_max_min_registry_and_local_cache_exports
    ImageCacheBoundary::JOBS.each do |workflow, id|
      %w[gha registry local].product(%w[min max]).each do |backend, mode|
        changed = Marshal.load(Marshal.dump(@documents))
        builds = changed[workflow]['jobs'][id]['steps'].select { |s| s.fetch('uses', '').start_with?('docker/build-push-action@') }
        builds.each do |build|
          build['with']['cache-to'] = "type=#{backend},mode=#{mode},ref=example.invalid/cache"
          assert_raises(RuntimeError) { ImageCacheBoundary.validate(changed) }
          build['with'].delete('cache-to')
        end
      end
    end
  end

  def test_explicit_and_post_job_uploads_cannot_export_layer_directories
    %w[actions/cache/save actions/cache actions/upload-artifact].each do |action|
      %w[success() always() failure()].each do |condition|
        changed = Marshal.load(Marshal.dump(@documents))
        changed['ci-omarchy']['jobs']['omarchy']['steps'] << {
          'uses' => action + '@fixture', 'if' => condition,
          'with' => {'path' => '${{ runner.temp }}/omarchy-layer-cache', 'key' => 'fixture'}
        }
        assert_raises(RuntimeError) { ImageCacheBoundary.validate(changed) }
      end
    end
  end

  def test_build_record_and_shell_export_bypasses_are_refused
    ImageCacheBoundary::JOBS.each do |workflow, id|
      changed = Marshal.load(Marshal.dump(@documents))
      changed[workflow]['jobs'][id]['env'].delete('DOCKER_BUILD_RECORD_UPLOAD')
      assert_raises(RuntimeError) { ImageCacheBoundary.validate(changed) }
      changed = Marshal.load(Marshal.dump(@documents))
      changed[workflow]['jobs'][id]['steps'] << {'run' => 'docker buildx build --cache-to=type=gha,mode=max .'}
      assert_raises(RuntimeError) { ImageCacheBoundary.validate(changed) }
    end
  end

  def test_discarded_prepared_credential_never_enters_the_exported_final_files
    assert ImageCacheBoundary.validate(@documents)
    Dir.mktmpdir('image-cache-boundary-') do |directory|
      context = File.join(directory, 'context')
      Dir.mkdir(context)
      marker = 'synthetic-' + SecureRandom.hex(32)
      File.write(File.join(context, 'private-fixture'), marker)
      File.write(File.join(context, 'public'), 'safe final payload')
      File.write(File.join(context, 'Dockerfile'), <<~DOCKERFILE)
        FROM scratch AS prepared
        COPY private-fixture /private-fixture
        COPY public /public
        FROM scratch AS final
        COPY --from=prepared /public /public
      DOCKERFILE
      %w[prepared final].each do |target|
        destination = File.join(directory, target)
        _output, status = Open3.capture2e('docker', 'buildx', 'build', '--network=none',
          '--provenance=false', '--sbom=false', '--target', target,
          '--output', "type=local,dest=#{destination}", context)
        assert status.success?, 'bounded local multi-stage fixture build failed'
      end
      assert_equal marker, File.read(File.join(directory, 'prepared/private-fixture'))
      assert_equal ['public'], Dir.children(File.join(directory, 'final'))
      assert_equal 'safe final payload', File.read(File.join(directory, 'final/public'))
    end
  end
end
