//! Build-generated constants share the publisher's explicit version declaration.

include!(concat!(env!("OUT_DIR"), "/runtime_versions.rs"));

#[cfg(test)]
#[path = "../build/runtime_versions.rs"]
mod generator;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_constants_match_the_declaration() {
        let source = include_str!("../runtime-versions.json");
        let value: serde_json::Value = serde_json::from_str(source).unwrap();
        assert_eq!(value["runtime_contract"], RUNTIME_CONTRACT_VERSION);
        assert_eq!(value["bootstrap"], RUNTIME_BOOTSTRAP_VERSION.to_string());
        assert_eq!(value["bootstrap"], RUNTIME_BOOTSTRAP_VERSION_LABEL);
        assert_eq!(value["home_layout"], RUNTIME_HOME_LAYOUT_VERSION);
        assert_eq!(
            generator::constants(source).unwrap(),
            include_str!(concat!(env!("OUT_DIR"), "/runtime_versions.rs"))
        );
    }

    #[test]
    fn malformed_or_ambiguous_versions_refuse_generation() {
        let source = include_str!("../runtime-versions.json");
        for bad in [
            source.replace("\"3\"", "3"),
            source.replace("\"3\"", "\"03\""),
            source.replace("\"3\"", "\"0\""),
            source.replace("\"3\"", "\"65536\""),
            source.replace("\"3\"", "\"3\\n\""),
            source.replace("versions-v1", "versions-v2"),
            source.replace("\"home_layout\":", "\"unknown\":"),
            source.replace("\"bootstrap\": \"1\",", ""),
            source.replace(
                "\"home_layout\": \"2\"",
                "\"home_layout\": \"2\", \"home_layout\": \"3\"",
            ),
            " ".repeat(1025),
        ] {
            assert!(generator::constants(&bad).is_err());
        }
    }
}
