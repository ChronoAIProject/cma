const MAX_IMAGE_REFERENCE_LENGTH: usize = 2_048;
pub(super) const SHA256_SEPARATOR: &str = "@sha256:";

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct PoolRollout {
    pub(crate) fixed_prewarm: [Option<String>; 2],
    pub(crate) pool_create_cutover_epoch: Option<uuid::Uuid>,
}

impl PoolRollout {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        fixed_native: bool,
        has_machine: bool,
    ) -> Result<Self, String> {
        let mut prewarm = |key| {
            lookup(key)?
                .map(|image| parse_pinned_image(key, &image))
                .transpose()
        };
        let fixed_prewarm = [
            prewarm("CHRONO_NATIVE_STANDARD_POOL_PREWARM_IMAGE")?,
            prewarm("CHRONO_NATIVE_MACHINE_POOL_PREWARM_IMAGE")?,
        ];
        if fixed_prewarm.iter().any(Option::is_some) && !fixed_native {
            return Err(
                "fixed-pin pool prewarm requires native allocation and fixed runtime pins".into(),
            );
        }
        if fixed_prewarm[1].is_some() && !has_machine {
            return Err("Machine pool prewarm requires an incumbent Machine runtime".into());
        }
        let pool_create_cutover_epoch = lookup("CHRONO_NATIVE_POOL_CREATE_CUTOVER_EPOCH")?
            .map(|value| {
                uuid::Uuid::parse_str(&value).map_err(|_| {
                    "CHRONO_NATIVE_POOL_CREATE_CUTOVER_EPOCH must be a UUID".to_owned()
                })
            })
            .transpose()?;
        Ok(Self {
            fixed_prewarm,
            pool_create_cutover_epoch,
        })
    }
}

pub(super) fn parse_pinned_image(key: &str, raw: &str) -> Result<String, String> {
    let valid = raw
        .rsplit_once(SHA256_SEPARATOR)
        .is_some_and(|(image, digest)| {
            raw.len() <= MAX_IMAGE_REFERENCE_LENGTH
                && !image.is_empty()
                && !image.contains('@')
                && !image.contains("://")
                && !image.starts_with(['.', '-', '_', '/', ':'])
                && !image.ends_with(['.', '-', '_', '/', ':'])
                && !image.contains("//")
                && image.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
                })
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        });
    if !valid {
        return Err(format!(
            "{key} must be an immutable OCI image reference ending in @sha256: and 64 lowercase hexadecimal digits"
        ));
    }
    Ok(raw.to_owned())
}
