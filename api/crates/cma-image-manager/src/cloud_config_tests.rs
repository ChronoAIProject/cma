use super::*;

fn configured(project: &str, location: &str, account: &str, registry: &str) -> Result<CloudBuild> {
    CloudBuild::new(
        project.into(),
        location.into(),
        account.into(),
        format!("gcr.io/cloud-builders/docker@sha256:{}", "a".repeat(64)),
        ImageRegistry::parse(registry.into())?,
        None,
    )
}

#[test]
fn alternate_configurations_bind_submission_observation_and_public_references() {
    let (_, mut build) = super::tests::fixture();
    for (project, location, region) in [
        ("example-build-one", "us-central1", "us"),
        ("example-build-two", "europe-west1", "europe"),
    ] {
        let repository = format!("{region}-docker.pkg.dev/{project}/runtime-images");
        let account =
            format!("projects/{project}/serviceAccounts/builder@{project}.iam.gserviceaccount.com");
        let executor = configured(project, location, &account, &repository).unwrap();
        assert_eq!(executor.collection(), format!("https://cloudbuild.googleapis.com/v1/projects/{project}/locations/{location}/builds"));
        let mut response = executor.request(&build).unwrap();
        let destination = format!("{repository}/{}:{}", build.image_id, build.id);
        assert_eq!(response["images"], json!([destination]));
        assert_eq!(response["serviceAccount"], account);
        response["id"] = json!(uuid::Uuid::new_v4());
        response["status"] = json!("SUCCESS");
        let digest = format!("sha256:{}", "b".repeat(64));
        response["results"] = json!({"images":[{"name":destination,"digest":digest}]});
        let observed = executor.observation(&build, &response).unwrap();
        build.digest = observed.digest;
        let expected = format!("{repository}/{}@{digest}", build.image_id);
        let view = crate::http::shapes::BuildView::new(build.clone(), &executor.registry);
        assert_eq!(view.reference.as_deref(), Some(expected.as_str()));
        let version = crate::http::shapes::VersionView::new(build.clone(), &executor.registry);
        assert_eq!(version.reference, view.reference);

        response["results"]["images"][0]["name"] = json!("elsewhere.invalid/image:tag");
        assert!(executor.observation(&build, &response).is_err());
        let stored = serde_json::to_value(&build).unwrap();
        assert!(stored.get("registry").is_none());
        assert_eq!(serde_json::from_value::<Build>(stored).unwrap(), build);
    }
}

#[test]
fn invalid_or_cross_project_settings_fail_before_any_cloud_request() {
    let project = "example-build-one";
    let account = "projects/example-build-one/serviceAccounts/builder@example-build-one.iam.gserviceaccount.com";
    let registry = "us-docker.pkg.dev/example-build-one/runtime-images";
    for invalid in [
        "",
        "short",
        "../project",
        "UPPERCASE",
        "trailing-",
        "with space",
    ] {
        assert!(configured(invalid, "us-central1", account, registry).is_err());
    }
    for invalid in ["", "../region", "region?query", " region", "region/"] {
        assert!(configured(project, invalid, account, registry).is_err());
    }
    for invalid in [
        "",
        "projects/another-project/serviceAccounts/builder@example-build-one.iam.gserviceaccount.com",
        "projects/example-build-one/serviceAccounts/builder@another-project.iam.gserviceaccount.com",
        "projects/example-build-one/serviceAccounts/../builder@example-build-one.iam.gserviceaccount.com",
        "projects/example-build-one/serviceAccounts/builder@example-build-one.iam.gserviceaccount.com\n",
    ] {
        assert!(configured(project, "us-central1", invalid, registry).is_err());
    }
    assert!(configured(
        project,
        "us-central1",
        account,
        "us-docker.pkg.dev/another-project/runtime-images",
    )
    .is_err());
    assert!(CloudBuild::new(
        project.into(),
        "us-central1".into(),
        account.into(),
        "gcr.io/cloud-builders/docker:latest".into(),
        ImageRegistry::parse(registry.into()).unwrap(),
        None,
    )
    .is_err());
}
