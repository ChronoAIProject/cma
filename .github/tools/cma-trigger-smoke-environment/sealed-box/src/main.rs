use std::env;
use std::io::{self, Read, Write};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use crypto_box::aead::OsRng;
use crypto_box::{PublicKey, SecretKey};
use zeroize::Zeroizing;

const PUBLIC_KEY_ENV: &str = "CMA_TRIGGER_SEAL_PUBLIC_KEY_B64";
// GitHub Actions accepts at most 48 KiB of plaintext for one secret.
const MAX_PLAINTEXT_BYTES: u64 = 48 * 1024;

fn decode_public_key(encoded: &str) -> Result<PublicKey, &'static str> {
    if encoded.is_empty() || encoded.len() > 128 || encoded.chars().any(char::is_whitespace) {
        return Err("public key must be one bounded canonical base64 value");
    }
    let decoded = BASE64
        .decode(encoded)
        .map_err(|_| "public key is not valid base64")?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| "public key must decode to exactly 32 bytes")?;
    if BASE64.encode(bytes) != encoded {
        return Err("public key is not canonical base64");
    }
    Ok(PublicKey::from(bytes))
}

fn read_plaintext() -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let mut input = Zeroizing::new(Vec::new());
    io::stdin()
        .take(MAX_PLAINTEXT_BYTES + 1)
        .read_to_end(&mut input)
        .map_err(|_| "plaintext could not be read from stdin")?;
    if input.is_empty() {
        return Err("plaintext must not be empty");
    }
    if input.len() as u64 > MAX_PLAINTEXT_BYTES {
        return Err("plaintext exceeds the sealed-box input limit");
    }
    Ok(input)
}

fn seal(public_key: &PublicKey, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
    public_key
        .seal(&mut OsRng, plaintext)
        .map_err(|_| "sealed-box encryption failed")
}

fn self_test() -> Result<(), &'static str> {
    let secret = SecretKey::from([0x5a; 32]);
    let public = secret.public_key();
    let plaintext = b"cma-trigger-sealed-box-self-test";
    let ciphertext = seal(&public, plaintext)?;
    if ciphertext.len() != plaintext.len() + 48 {
        return Err("sealed-box overhead is not libsodium compatible");
    }
    let opened = secret
        .unseal(&ciphertext)
        .map_err(|_| "sealed-box self-test could not decrypt")?;
    if opened != plaintext {
        return Err("sealed-box self-test plaintext mismatch");
    }
    Ok(())
}

fn run() -> Result<(), &'static str> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args == ["--self-test"] {
        self_test()?;
        println!("sealed-box self-test passed");
        return Ok(());
    }
    if !args.is_empty() {
        return Err("usage: cma-trigger-sealed-box [--self-test]");
    }
    let encoded =
        env::var(PUBLIC_KEY_ENV).map_err(|_| "CMA_TRIGGER_SEAL_PUBLIC_KEY_B64 is required")?;
    let public_key = decode_public_key(&encoded)?;
    let plaintext = read_plaintext()?;
    let ciphertext = seal(&public_key, &plaintext)?;
    let output = BASE64.encode(ciphertext);
    io::stdout()
        .write_all(output.as_bytes())
        .and_then(|_| io::stdout().write_all(b"\n"))
        .map_err(|_| "ciphertext could not be written")?;
    Ok(())
}

fn main() {
    if let Err(message) = run() {
        eprintln!("cma-trigger-sealed-box: {message}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libsodium_sealed_box_roundtrip() {
        self_test().expect("sealed-box roundtrip");
    }

    #[test]
    fn public_key_requires_canonical_exact_base64() {
        let secret = SecretKey::from([0x31; 32]);
        let encoded = BASE64.encode(secret.public_key().as_bytes());
        assert!(decode_public_key(&encoded).is_ok());
        assert!(decode_public_key(&format!("{encoded}\n")).is_err());
        assert!(decode_public_key("AA==").is_err());
    }
}
