use super::{
    CacheDecodeError, SecretValue, SubscriptionError, SubscriptionTokens, MAX_CACHE_BYTES,
};
use layer3_adapter::ingress_budget::{
    sizing, AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy, SharedOwned,
};

impl From<AdmissionRefusal> for CacheDecodeError {
    fn from(error: AdmissionRefusal) -> Self {
        Self::Refused(error)
    }
}

impl SubscriptionTokens {
    /// The cache reader must own the plaintext before passing it here.
    /// Parsing and returned credential copies have separate allocation custody.
    pub(crate) fn from_native_cache_admitted(
        cache: &SharedOwned<SecretValue>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SharedOwned<Self>, CacheDecodeError> {
        if !cache.shares_custody_domain(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "subscription cache ownership",
            }
            .into());
        }
        let bytes = cache.expose().len();
        if bytes > MAX_CACHE_BYTES {
            return Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount));
        }
        let mut storage = policy.admit(IngressClass::Data, decode_plan(bytes)?)?;
        let tokens = Self::from_native_cache(cache).map_err(CacheDecodeError::Account)?;
        let retained = AllocationPlan::checked(
            "subscription token result",
            &[
                sizing::shared_owner_bytes::<Self>(),
                // from_native_cache clones a String, whose new capacity is len.
                tokens.access_token.expose().len(),
                tokens.account_id.capacity(),
                tokens.plan_type.as_ref().map_or(0, String::capacity),
            ],
        )?;
        let result = storage.split_admitted(retained)?.initialize(|| tokens);
        // Parsing, decoding and diagnostic scratch is gone before reuse.
        drop(storage);
        Ok(result)
    }
}

fn decode_plan(bytes: usize) -> Result<AllocationPlan, AdmissionRefusal> {
    let invalid = || AdmissionRefusal::InvalidPlan {
        phase: "subscription cache decode layout",
    };
    let mul = |value: usize, count| value.checked_mul(count).ok_or_else(invalid);
    let add = |value: usize, extra| value.checked_add(extra).ok_or_else(invalid);
    // serde_json 1.0.154 deserializes these fixed structs without Content/maps:
    // native strings, claims strings and final copies each total at most input.
    // Each slice parser's scratch growth overlaps at most input+8 old bytes and
    // twice that in new capacity, including ignored nested values.
    let strings = mul(bytes, 3)?;
    let scratch = mul(add(bytes, 8)?, 6)?;
    // Base64 0.22.1 allocates its rounded decode estimate, at most input+3.
    let decoded = add(bytes, 3)?;
    // The enabled float_roundtrip fallback caps its mantissa at 769 digits.
    // Its two bigints plus multiplication/growth scratch fit 64 KiB, including
    // wrong numeric types that will ultimately be refused by the visitor.
    let numeric = 64 * 1024;
    // Serde's single failure may debug-escape a whole input string (<=10 bytes
    // per byte), plus <=128 fixed schema/number characters. Formatting growth
    // and into_boxed_str shrink can overlap. Each ErrorImpl fits eight usize
    // words; fix_position can overlap the original and replacement boxes.
    let diagnostic = mul(add(mul(bytes, 10)?, 128)?, 3)?;
    AllocationPlan::checked(
        "subscription cache decode",
        &[
            strings,
            scratch,
            decoded,
            numeric,
            diagnostic,
            16 * size_of::<usize>(),
            sizing::shared_owner_bytes::<SubscriptionTokens>(),
        ],
    )
}

#[cfg(test)]
#[path = "subscription_tokens_ingress_tests.rs"]
mod tests;
