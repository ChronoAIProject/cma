# frozen_string_literal: true

# Final-image qualification cannot authorize bytes from discarded build stages.
module ImageCacheBoundary
  JOBS = { 'ci-images' => 'images', 'ci-omarchy' => 'omarchy', 'publish' => 'images' }.freeze

  def self.validate(documents)
    JOBS.each do |workflow, id|
      job = documents.fetch(workflow).fetch('jobs').fetch(id)
      raise 'automatic build record upload enabled' unless job.dig('env', 'DOCKER_BUILD_RECORD_UPLOAD') == 'false'
      job.fetch('steps').each do |step|
        action = step.fetch('uses', '')
        settings = step.fetch('with', {})
        if action.start_with?('docker/build-push-action@')
          raise 'image layer cache export enabled' if settings.key?('cache-to')
          raise 'unscanned registry export enabled' unless settings['push'] == false
          outputs = settings.fetch('outputs', '')
          raise 'unscanned image exporter enabled' unless outputs.empty? || outputs == 'type=cacheonly' ||
            outputs == 'type=oci,dest=${{ steps.skopeo.outputs.output_dir }}/candidate.oci.tar'
          override = step.fetch('env', {})['DOCKER_BUILD_RECORD_UPLOAD']
          raise 'automatic build record upload override' unless override.nil? || override == 'false'
        end
        # The API Cargo bridge retains a separately bounded dependency-only archive.
        # No layer directory may be added to its upload path or any post-job saver.
        if action.start_with?('actions/cache@', 'actions/cache/save@')
          raise 'unqualified image cache upload enabled' unless workflow == 'ci-images' &&
            step['name'] == 'Save accepted API Cargo cache archive' &&
            settings['path'] == '${{ runner.temp }}/cma-api-cargo-cache/data'
        end
        if action.start_with?('actions/upload-artifact@')
          path = settings.fetch('path', '')
          raise 'image layer cache artifact upload enabled' if path.include?('layer-cache') ||
            path == '${{ runner.temp }}' || path == '${{ runner.temp }}/' || path.include?('**')
        end
        raise 'shell image cache export enabled' if step.fetch('run', '').match?(/--cache-to(?:=|\s)/)
      end
    end
    true
  end
end
