mod schema;
use schema::{Check, Direction};
use serde_json::Value;

pub fn errors(old: &Value, new: &Value) -> Vec<String> {
    let mut check = Check {
        old,
        new,
        errors: Vec::new(),
    };
    if old["servers"] != new["servers"] {
        check.fail("servers", "changed protocol base URL");
    }
    if old.pointer("/components/securitySchemes") != new.pointer("/components/securitySchemes") {
        // Additions are allowed, but retained schemes may not change.
        if let Some(schemes) = old
            .pointer("/components/securitySchemes")
            .and_then(Value::as_object)
        {
            for (name, scheme) in schemes {
                if new["components"]["securitySchemes"][name] != *scheme {
                    check.fail(name, "changed security scheme");
                }
            }
        }
    }
    for (path, item) in old["paths"].as_object().expect("snapshot paths") {
        let Some(current) = new["paths"].get(path) else {
            check.fail(path, "removed path");
            continue;
        };
        for method in [
            "get", "post", "put", "patch", "delete", "head", "options", "trace",
        ] {
            let Some(operation) = item.get(method) else {
                continue;
            };
            let location = format!("{} {path}", method.to_uppercase());
            let Some(updated) = current.get(method) else {
                check.fail(&location, "removed method");
                continue;
            };
            for key in ["operationId", "security"] {
                if operation[key] != updated[key] {
                    check.fail(&location, &format!("changed {key}"));
                }
            }
            check.parameters(item, current, &location);
            check.parameters(operation, updated, &location);
            let old_body = &operation["requestBody"];
            let new_body = &updated["requestBody"];
            if old_body["required"] != true && new_body["required"] == true {
                check.fail(&location, "request body became required");
            }
            check.content(
                old_body,
                new_body,
                &format!("{location} request"),
                Direction::Request,
            );
            if let Some(responses) = operation["responses"].as_object() {
                for (status, response) in responses {
                    let location = format!("{location} response {status}");
                    let Some(updated) = updated["responses"].get(status) else {
                        check.fail(&location, "removed response");
                        continue;
                    };
                    check.content(response, updated, &location, Direction::Response);
                }
            }
        }
    }
    // This extension is provider-facing and is not an HTTP operation on CMAEG.
    if let Some(snapshot) = old.pointer("/components/schemas/EventContextSnapshot") {
        check.schema(
            snapshot,
            &new["components"]["schemas"]["EventContextSnapshot"],
            "EventContextSnapshot",
            Direction::Request,
            0,
        );
    }
    check.errors.sort();
    check.errors.dedup();
    check.errors
}

impl Check<'_> {
    fn content(&mut self, old: &Value, new: &Value, path: &str, direction: Direction) {
        let old = Self::resolve(self.old, old);
        let new = Self::resolve(self.new, new);
        if let Some(content) = old["content"].as_object() {
            for (media, body) in content {
                let location = format!("{path} {media}");
                if let Some(current) = new["content"].get(media) {
                    self.schema(&body["schema"], &current["schema"], &location, direction, 0);
                } else {
                    self.fail(&location, "removed media type");
                }
            }
        }
    }

    fn parameters(&mut self, old: &Value, new: &Value, path: &str) {
        let empty = Vec::new();
        let old = old["parameters"].as_array().unwrap_or(&empty);
        let new = new["parameters"].as_array().unwrap_or(&empty);
        for old in old.iter().map(|value| Self::resolve(self.old, value)) {
            let location = format!("{path} {} parameter {}", old["in"], old["name"]);
            let current = new
                .iter()
                .map(|value| Self::resolve(self.new, value))
                .find(|value| value["in"] == old["in"] && value["name"] == old["name"]);
            if let Some(current) = current {
                if old["required"] != true && current["required"] == true {
                    self.fail(&location, "parameter became required");
                }
                for key in ["style", "explode", "allowReserved"] {
                    if old[key] != current[key] {
                        self.fail(&location, &format!("changed {key}"));
                    }
                }
                self.schema(
                    &old["schema"],
                    &current["schema"],
                    &location,
                    Direction::Request,
                    0,
                );
            } else {
                self.fail(&location, "removed parameter");
            }
        }
        for current in new.iter().map(|value| Self::resolve(self.new, value)) {
            if current["required"] == true
                && !old
                    .iter()
                    .map(|value| Self::resolve(self.old, value))
                    .any(|value| value["in"] == current["in"] && value["name"] == current["name"])
            {
                self.fail(path, &format!("new required parameter {}", current["name"]));
            }
        }
    }
}
