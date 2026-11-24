use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::{
    rand::SystemRandom,
    signature::{KeyPair, RsaKeyPair, RSA_PKCS1_SHA256},
};
use serde_json::{json, Value};
use std::{process::Command, sync::OnceLock};

// Test-only RSA keys are generated in memory. No private fixture is committed.
pub struct Signer(RsaKeyPair);
impl Signer {
    fn generate() -> Self {
        let output = Command::new("openssl")
            .args(["genrsa", "2048"])
            .output()
            .expect("openssl generates an ephemeral test RSA key");
        assert!(output.status.success(), "test RSA generation failed");
        let pem = String::from_utf8(output.stdout).unwrap();
        let encoded: String = pem.lines().filter(|line| !line.starts_with('-')).collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let key = if pem.starts_with("-----BEGIN PRIVATE KEY-----") {
            RsaKeyPair::from_pkcs8(&der)
        } else {
            RsaKeyPair::from_der(&der)
        };
        Self(key.unwrap())
    }
    pub fn primary() -> &'static Self {
        static SIGNER: OnceLock<Signer> = OnceLock::new();
        SIGNER.get_or_init(Self::generate)
    }
    pub fn rotated() -> &'static Self {
        static SIGNER: OnceLock<Signer> = OnceLock::new();
        SIGNER.get_or_init(Self::generate)
    }
    pub fn token(&self, claims: &Value, kid: &str) -> String {
        self.raw(
            &json!({"alg":"RS256","typ":"JWT","kid":kid}).to_string(),
            &claims.to_string(),
        )
    }
    pub(super) fn raw(&self, header: &str, body: &str) -> String {
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header),
            URL_SAFE_NO_PAD.encode(body)
        );
        let mut signature = vec![0; self.0.public().modulus_len()];
        self.0
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signed.as_bytes(),
                &mut signature,
            )
            .unwrap();
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
    }
    pub fn key(&self, kid: &str) -> Value {
        let mut der = self.0.public_key().as_ref();
        let mut sequence = tlv(&mut der, 0x30);
        let n = tlv(&mut sequence, 0x02);
        let e = tlv(&mut sequence, 0x02);
        json!({"kty":"RSA","kid":kid,"alg":"RS256","use":"sig","key_ops":["verify"],
            "n":URL_SAFE_NO_PAD.encode(n.strip_prefix(&[0]).unwrap_or(n)),"e":URL_SAFE_NO_PAD.encode(e)})
    }
}

fn tlv<'a>(input: &mut &'a [u8], tag: u8) -> &'a [u8] {
    assert_eq!(input[0], tag);
    let head = input[1];
    *input = &input[2..];
    let size = if head < 128 {
        head as usize
    } else {
        let bytes = (head & 127) as usize;
        let size = input[..bytes]
            .iter()
            .fold(0_usize, |n, b| (n << 8) | usize::from(*b));
        *input = &input[bytes..];
        size
    };
    let value = &input[..size];
    *input = &input[size..];
    value
}
