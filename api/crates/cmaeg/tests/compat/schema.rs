use serde_json::Value;

#[derive(Clone, Copy)]
pub enum Direction {
    Request,
    Response,
}

pub struct Check<'a> {
    pub old: &'a Value,
    pub new: &'a Value,
    pub errors: Vec<String>,
}

impl Check<'_> {
    pub fn fail(&mut self, path: &str, reason: &str) {
        self.errors.push(format!("{path}: {reason}"));
    }

    pub fn resolve<'a>(document: &'a Value, mut schema: &'a Value) -> &'a Value {
        for _ in 0..64 {
            let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
                return schema;
            };
            schema = document
                .pointer(reference.strip_prefix('#').expect("local schema reference"))
                .unwrap_or_else(|| panic!("unresolved schema reference {reference}"));
        }
        panic!("cyclic schema aliases");
    }

    pub fn schema(
        &mut self,
        old: &Value,
        new: &Value,
        path: &str,
        direction: Direction,
        depth: usize,
    ) {
        if depth > 64 {
            self.fail(
                path,
                "schema recursion exceeds compatibility inspection limit",
            );
            return;
        }
        let old = Self::resolve(self.old, old);
        let new = Self::resolve(self.new, new);
        for key in ["type", "format", "discriminator", "const"] {
            if old.get(key) != new.get(key) {
                self.fail(
                    path,
                    &format!("changed {key}: {} -> {}", old[key], new[key]),
                );
            }
        }
        if let Some(values) = old["enum"].as_array() {
            for value in values {
                if !new["enum"]
                    .as_array()
                    .is_some_and(|values| values.contains(value))
                {
                    self.fail(path, &format!("removed enum value {value}"));
                }
            }
        } else if new.get("enum").is_some() {
            self.fail(path, "added enum constraint");
        }
        let empty = Vec::new();
        let old_required = old["required"].as_array().unwrap_or(&empty);
        let new_required = new["required"].as_array().unwrap_or(&empty);
        let (before, after, reason) = match direction {
            Direction::Request => (new_required, old_required, "new required request property"),
            Direction::Response => (
                old_required,
                new_required,
                "response property no longer required",
            ),
        };
        for value in before {
            if !after.contains(value) {
                self.fail(path, &format!("{reason} {value}"));
            }
        }
        if let Some(properties) = old["properties"].as_object() {
            for (name, schema) in properties {
                let path = format!("{path}.{name}");
                if let Some(current) = new["properties"].get(name) {
                    self.schema(schema, current, &path, direction, depth + 1);
                } else {
                    self.fail(&path, "removed property");
                }
            }
        }
        for key in ["items", "propertyNames", "additionalProperties"] {
            match (old.get(key), new.get(key)) {
                (Some(a), Some(b)) if a.is_object() && b.is_object() => {
                    self.schema(a, b, &format!("{path}.{key}"), direction, depth + 1)
                }
                (a, b) if a != b => self.fail(path, &format!("changed {key} constraint")),
                _ => {}
            }
        }
        for key in ["oneOf", "anyOf", "allOf"] {
            self.variants(old, new, key, path, direction, depth);
        }
        for key in [
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "minLength",
            "maxLength",
            "minItems",
            "maxItems",
            "minProperties",
            "maxProperties",
            "pattern",
            "multipleOf",
            "uniqueItems",
            "not",
        ] {
            if old.get(key) != new.get(key) {
                // Conservative for constraints: reviewers must establish wire compatibility
                // before broadening the comparator to support a new kind of change.
                self.fail(path, &format!("changed {key} constraint"));
            }
        }
    }

    fn variants(
        &mut self,
        old: &Value,
        new: &Value,
        key: &str,
        path: &str,
        direction: Direction,
        depth: usize,
    ) {
        let Some(variants) = old[key].as_array() else {
            if new.get(key).is_some() {
                self.fail(path, &format!("added {key} constraint"));
            }
            return;
        };
        let Some(current) = new[key].as_array() else {
            self.fail(path, &format!("removed {key} variants"));
            return;
        };
        if key == "allOf" && current.len() != variants.len() {
            self.fail(path, "changed allOf constraints");
        }
        for (index, variant) in variants.iter().enumerate() {
            let mut closest: Option<Vec<String>> = None;
            for candidate in current {
                let mut trial = Check {
                    old: self.old,
                    new: self.new,
                    errors: Vec::new(),
                };
                trial.schema(
                    variant,
                    candidate,
                    &format!("{path}.{key}[{index}]"),
                    direction,
                    depth + 1,
                );
                if closest
                    .as_ref()
                    .is_none_or(|errors| trial.errors.len() < errors.len())
                {
                    closest = Some(trial.errors);
                }
            }
            match closest {
                Some(errors) if errors.is_empty() => {}
                Some(errors) => self.errors.extend(errors),
                None => self.fail(path, &format!("removed {key}[{index}] variant")),
            }
        }
    }
}
