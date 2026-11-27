require 'yaml'

module WorkflowYAML
  def self.load_file(path)
    # The Web track shares job setup with GitHub Actions YAML aliases.
    YAML.safe_load(File.read(path), aliases: File.basename(path, File.extname(path)) == 'ci-web')
  end
end
