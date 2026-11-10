use super::{child, configmap_value, container, item, lines, value, CONFIGMAP, DEPLOYMENT};
const KEYS: [&str; 2] = ["CMAEG_OWNER_KEY_SERVICE_ID", "CMAEG_OWNER_KEY_CUTOVER_ID"];
const SECRET: &str = "cma-event-gateway-owner-keys";

fn contract(configmap: &str, deployment: &str) -> Result<(), &'static str> {
    let rows = lines(deployment);
    let owner = container(&rows, "cmaeg").map_err(|_| "container missing")?;
    let env = child(&rows, owner, "env").ok_or("env missing")?;
    for key in KEYS {
        if configmap_value(configmap, key).is_some() {
            return Err("ConfigMap contains identifier");
        }
        let index = item(&rows, env, key).ok_or("env entry missing")?;
        if rows
            .iter()
            .filter(|line| line.text == format!("- name: {key}"))
            .count()
            != 1
        {
            return Err("env entry duplicated");
        }
        if child(&rows, index, "value").is_some() {
            return Err("literal env forbidden");
        }
        let reference = child(&rows, index, "valueFrom")
            .and_then(|index| child(&rows, index, "secretKeyRef"))
            .ok_or("Secret reference missing")?;
        for (field, expected) in [("name", SECRET), ("key", key), ("optional", "true")] {
            let actual = child(&rows, reference, field).and_then(|i| value(&rows[i], field));
            if actual != Some(expected) {
                return Err("Secret reference mismatch");
            }
        }
    }
    Ok(())
}
#[test]
fn owner_identifiers_are_secret_references_and_never_configmap_placeholders() {
    assert_eq!(contract(CONFIGMAP, DEPLOYMENT), Ok(()));
    let secret = include_str!("../../../../../infra/cmaeg/secret.yaml");
    assert!(secret.contains(&format!("name: {SECRET}")));
    for key in KEYS {
        assert!(secret.contains(&format!("#   {key} ")));
        for placeholder in ["", "fixture"] {
            let bad = format!("{CONFIGMAP}  {key}: \"{placeholder}\"\n");
            assert!(contract(&bad, DEPLOYMENT).is_err());
        }
        let bad = DEPLOYMENT.replace(&format!("key: {key}"), "key: WRONG_KEY");
        assert!(contract(CONFIGMAP, &bad).is_err());
    }
    for bad in [
        DEPLOYMENT.replace(SECRET, "wrong-secret"),
        DEPLOYMENT.replace("secretKeyRef:", "configMapKeyRef:"),
        DEPLOYMENT.replace("optional: true", "optional: false"),
    ] {
        assert!(contract(CONFIGMAP, &bad).is_err());
    }
}
