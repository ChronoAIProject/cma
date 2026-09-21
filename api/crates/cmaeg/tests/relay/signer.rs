//! Keys are generated in memory at runtime; no private key fixture is retained.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::{
    rand::SystemRandom,
    signature::{KeyPair, RsaKeyPair, RSA_PKCS1_SHA256},
};
use serde_json::{json, Value};
use std::{process::Command, sync::OnceLock};

pub struct Signer(RsaKeyPair);
impl Signer {
    pub fn shared() -> &'static Self {
        static SIGNER: OnceLock<Signer> = OnceLock::new();
        SIGNER.get_or_init(|| {
            let output = Command::new("openssl")
                .args(["genrsa", "-traditional", "2048"])
                .output()
                .expect("openssl generates ephemeral test RSA key");
            assert!(output.status.success());
            let pem = String::from_utf8(output.stdout).unwrap();
            let encoded: String = pem.lines().filter(|line| !line.starts_with('-')).collect();
            use base64::engine::general_purpose::STANDARD;
            Self(RsaKeyPair::from_der(&STANDARD.decode(encoded).unwrap()).unwrap())
        })
    }
    pub fn token(&self, claims: &Value) -> String {
        self.with_header(claims, json!({"alg":"RS256","typ":"JWT","kid":"test-rsa"}))
    }
    pub fn with_header(&self, claims: &Value, header: Value) -> String {
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
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
    pub fn jwks(&self) -> Value {
        let mut der = self.0.public_key().as_ref();
        let mut sequence = tlv(&mut der, 0x30);
        let n = tlv(&mut sequence, 0x02);
        let e = tlv(&mut sequence, 0x02);
        let n = n.strip_prefix(&[0]).unwrap_or(n);
        json!({"keys":[{"kty":"RSA","kid":"test-rsa","alg":"RS256","n":URL_SAFE_NO_PAD.encode(n),"e":URL_SAFE_NO_PAD.encode(e)}]})
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
        let value = input[..bytes]
            .iter()
            .fold(0_usize, |n, b| (n << 8) | usize::from(*b));
        *input = &input[bytes..];
        value
    };
    let value = &input[..size];
    *input = &input[size..];
    value
}
