use super::*;
use serde::Deserialize;
#[derive(Deserialize)]
struct Input {
    id: String,
    version: u32,
    fields: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct Vector {
    template: Input,
    message: String,
}

#[test]
fn shared_compilation_vectors() {
    let vectors: Vec<Vector> =
        serde_json::from_str(include_str!("task_template_vectors.json")).unwrap();
    for vector in vectors {
        let input = vector.template;
        assert_eq!(
            compile_task_template(&input.id, input.version, &input.fields).unwrap(),
            vector.message
        );
        for key in input.fields.keys() {
            let mut fields = input.fields.clone();
            fields.remove(key);
            assert_eq!(
                compile_task_template(&input.id, input.version, &fields)
                    .unwrap_err()
                    .code,
                "task_template_field_required"
            );
            fields.insert(key.clone(), " \n\u{85} ".into());
            assert_eq!(
                compile_task_template(&input.id, input.version, &fields)
                    .unwrap_err()
                    .code,
                "task_template_field_required"
            );
        }
    }
}

#[test]
fn invalid_template_identity_fields_placeholders_and_size_are_refused() {
    let fields = BTreeMap::from([("topic".into(), "[topic]".into())]);
    for (id, version, code) in [
        ("unknown", 1, "task_template_id_unknown"),
        ("research", 3, "task_template_version_unsupported"),
        ("research", 1, "task_template_field_placeholder"),
    ] {
        assert_eq!(
            compile_task_template(id, version, &fields)
                .unwrap_err()
                .code,
            code
        );
    }
    let mut fields = BTreeMap::from([
        ("topic".into(), "synthetic".into()),
        ("injected".into(), "private input".into()),
    ]);
    let error = compile_task_template("research", 1, &fields).unwrap_err();
    assert_eq!(error.code, "task_template_field_unknown");
    assert!(!error.to_string().contains("private input"));
    fields.remove("injected");
    let overhead = compile_task_template("research", 1, &fields).unwrap().len() - "synthetic".len();
    fields.insert(
        "topic".into(),
        "x".repeat(super::super::MAX_FIRST_MESSAGE_BYTES - overhead),
    );
    assert_eq!(
        compile_task_template("research", 1, &fields).unwrap().len(),
        super::super::MAX_FIRST_MESSAGE_BYTES
    );
    fields.get_mut("topic").unwrap().push('é');
    assert_eq!(
        compile_task_template("research", 1, &fields)
            .unwrap_err()
            .code,
        "task_template_message_too_large"
    );
}
