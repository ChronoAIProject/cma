#[tokio::main]
async fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--print-openapi"] {
        print!("{}", cmaeg::openapi::snapshot());
        return;
    }
    if !arguments.is_empty() {
        eprintln!("usage: cmaeg [--print-openapi]");
        std::process::exit(2);
    }
    cmaeg::init_logging();
    if let Err(error) = cmaeg::run().await {
        tracing::error!(%error, "CMAEG stopped");
        std::process::exit(1);
    }
}
