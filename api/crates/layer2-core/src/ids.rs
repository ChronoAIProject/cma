//! Opaque identifiers shared by the retained core foundations.

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Wrap an identifier supplied by an authoritative upstream.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Borrow the underlying identifier.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(SubjectId, "Identifier asserted by the identity provider.");
string_id!(
    ServiceId,
    "Identifier of a service exposed by the identity platform."
);
string_id!(
    SkillId,
    "Identifier of a skill exposed by the skill catalogue."
);

macro_rules! generated_id {
    ($name:ident, $prefix:literal, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Mint a fresh identifier with a resource-specific prefix.
            pub fn generate() -> Self {
                Self(format!(
                    concat!($prefix, "{}"),
                    uuid::Uuid::new_v4().simple()
                ))
            }

            /// Borrow the underlying identifier.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

generated_id!(SandboxId, "sbx_", "Identifier of a CMA Sandbox.");
generated_id!(WorkspaceId, "wks_", "Identifier of a CMA Workspace.");
generated_id!(WorktreeId, "wkt_", "Identifier of a CMA Worktree.");
generated_id!(AgentId, "agt_", "Identifier of a CMA Agent.");
generated_id!(AgentProfileId, "agp_", "Identifier of an Agent Profile.");
generated_id!(
    WorkspaceProfileId,
    "wkp_",
    "Identifier of a Workspace Profile."
);
generated_id!(SandboxProfileId, "sbp_", "Identifier of a Sandbox Profile.");
generated_id!(TriggerId, "trg_", "Identifier of a CMA Trigger.");
generated_id!(
    PublicTriggerId,
    "pub_",
    "Stable public identifier of a CMA Trigger."
);
generated_id!(PlacementId, "plc_", "Identifier of a Trigger placement.");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_keep_their_type_and_wire_value() {
        let subject = SubjectId::new("subject-a");
        let service = ServiceId::new("api-github");
        let skill = SkillId::new("lean-research");

        assert_eq!(subject.as_str(), "subject-a");
        assert_eq!(service.to_string(), "api-github");
        assert_eq!(serde_json::to_string(&skill).unwrap(), "\"lean-research\"");
    }

    #[test]
    fn generated_resource_identifiers_use_distinct_prefixes() {
        assert!(SandboxId::generate().as_str().starts_with("sbx_"));
        assert!(WorkspaceId::generate().as_str().starts_with("wks_"));
        assert!(WorktreeId::generate().as_str().starts_with("wkt_"));
        assert!(AgentId::generate().as_str().starts_with("agt_"));
        assert!(AgentProfileId::generate().as_str().starts_with("agp_"));
        assert!(WorkspaceProfileId::generate().as_str().starts_with("wkp_"));
        assert!(SandboxProfileId::generate().as_str().starts_with("sbp_"));
        assert!(TriggerId::generate().as_str().starts_with("trg_"));
        assert!(PublicTriggerId::generate().as_str().starts_with("pub_"));
        assert!(PlacementId::generate().as_str().starts_with("plc_"));
    }
}
