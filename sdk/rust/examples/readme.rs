use cma_sdk::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dry_run = std::env::var_os("CMA_SDK_EXAMPLE_DRY_RUN").is_some();
    let base_url = if dry_run {
        "https://example.invalid".to_string()
    } else {
        std::env::var("CMA_BASE_URL")?
    };
    let client = Client::builder(base_url)?.build()?;
    if dry_run {
        drop(client.read_conformance());
        return Ok(());
    }

    let deployment = client.read_conformance().await?;
    println!("{}: {}", deployment.service, deployment.status);
    Ok(())
}
