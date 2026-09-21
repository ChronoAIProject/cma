#[tokio::main]
async fn main() {
    cmaeg::init_logging();
    if let Err(error) = cmaeg::run().await {
        tracing::error!(%error, "CMAEG stopped");
        std::process::exit(1);
    }
}
