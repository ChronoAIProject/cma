//! Inline CMA mark geometry for cacheable, self-contained SVG badges.

/// Copied from `web/src/assets/brand/cma-mark.svg`.
///
/// The API container is built from `api/` alone, so this cannot use
/// `include_str!` to reach into the web image's source tree.
pub(super) const CMA_MARK: &str = r#"<path d="M71 71 A30 30 0 1 0 29 71" fill="none" stroke="currentColor" stroke-width="11" stroke-linecap="round"/>
<path d="M71 71 L61 59 L50 71 L39 59 L29 71" fill="none" stroke="currentColor" stroke-width="10" stroke-linecap="round" stroke-linejoin="round"/>
<rect x="30" y="38" width="17" height="12" rx="4" fill="currentColor"/>
<rect x="53" y="38" width="17" height="12" rx="4" fill="currentColor"/>
<path d="M46 44 L54 44" fill="none" stroke="currentColor" stroke-width="4"/>"#;
