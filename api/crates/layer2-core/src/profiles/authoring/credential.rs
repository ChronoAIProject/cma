use zeroize::Zeroizing;

/// Transient authority for an owner-scoped authoring lookup; never persisted.
pub struct AuthoringCredential(Zeroizing<String>);

impl AuthoringCredential {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn value(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for AuthoringCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AuthoringCredential(<redacted>)")
    }
}
