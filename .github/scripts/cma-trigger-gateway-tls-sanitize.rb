#!/usr/bin/env ruby
# frozen_string_literal: true

require "base64"
require "digest"
require "json"
require "openssl"

def fail_closed
  warn "cma-trigger Gateway TLS sanitizer: Secret validation failed"
  exit 1
end

begin
  output_path = ARGV.fetch(0)
  raise ArgumentError unless ARGV.length == 1 && output_path.start_with?("/")

  secret = JSON.parse($stdin.read)
  data = secret.fetch("data")
  raise ArgumentError unless data.is_a?(Hash) && data.keys.sort == %w[tls.crt tls.key]

  certificate_source = Base64.strict_decode64(data.fetch("tls.crt"))
  key_pem = Base64.strict_decode64(data.fetch("tls.key"))
  certificate_blocks = []
  certificate_remainder = certificate_source
  certificate_pattern = /\A[[:space:]]*(-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----)/m
  while (match = certificate_pattern.match(certificate_remainder))
    certificate_blocks << match[1]
    certificate_remainder = certificate_remainder.byteslice(match[0].bytesize..-1)
  end
  raise ArgumentError unless certificate_remainder.match?(/\A[[:space:]]*\z/)

  certificates = certificate_blocks.map { |block| OpenSSL::X509::Certificate.new(block) }
  raise ArgumentError if certificates.empty?

  certificate_pem = certificates.map(&:to_pem).join
  leaf = certificates.fetch(0)
  key = OpenSSL::PKey.read(key_pem)
  public_key = leaf.public_key
  challenge = OpenSSL::Random.random_bytes(32)
  digest = OpenSSL::Digest::SHA256.new
  signature = key.sign(digest, challenge)
  raise ArgumentError unless public_key.verify(digest, signature, challenge)
  public_spki = Digest::SHA256.hexdigest(public_key.to_der)

  sanitized = {
    "apiVersion" => secret["apiVersion"],
    "kind" => secret["kind"],
    "metadata" => {
      "name" => secret.dig("metadata", "name"),
      "namespace" => secret.dig("metadata", "namespace"),
      "uid" => secret.dig("metadata", "uid"),
      "resourceVersion" => secret.dig("metadata", "resourceVersion"),
      "deletionTimestamp" => secret.dig("metadata", "deletionTimestamp"),
    },
    "type" => secret["type"],
    "dataKeys" => data.keys.sort,
    "certificatePem" => certificate_pem,
    "keyPresent" => true,
    "publicSpkiSha256" => public_spki,
  }

  File.open(output_path, File::WRONLY | File::CREAT | File::EXCL, 0o600) do |file|
    file.write(JSON.generate(sanitized))
    file.write("\n")
    file.fsync
  end
rescue StandardError
  fail_closed
end
