#[tokio::main]
async fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("--import-legacy-records")
        && arguments.len() == 2
    {
        if let Err(error) = import_records(&arguments[1]).await {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments == ["--print-openapi"] {
        print!("{}", cmaeg::openapi::snapshot());
        return;
    }
    if matches!(
        arguments.first().map(String::as_str),
        Some("--migrate" | "--print-runtime-role" | "--print-migration-role")
    ) && arguments.len() == 1
    {
        if let Err(error) = database_command(&arguments[0]).await {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if !arguments.is_empty() {
        eprintln!(
            "usage: cmaeg [--print-openapi|--migrate|--print-runtime-role|--print-migration-role]"
        );
        std::process::exit(2);
    }
    cmaeg::init_logging();
    if let Err(error) = cmaeg::run().await {
        tracing::error!(%error, "CMAEG stopped");
        std::process::exit(1);
    }
}

async fn database_command(command: &str) -> Result<(), Box<dyn std::error::Error>> {
    let database = std::env::var(cmaeg::CMAEG_MONGODB_DATABASE)?;
    if command != "--migrate" {
        println!(
            "{}",
            cmaeg::records::mongo::role(&database, command == "--print-migration-role")
        );
        return Ok(());
    }
    let client = mongodb::Client::with_uri_str(std::env::var(cmaeg::CMAEG_MONGODB_URI)?).await?;
    cmaeg::records::mongo::migrate(&client.database(&database)).await?;
    Ok(())
}

async fn import_records(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let database = std::env::var(cmaeg::CMAEG_MONGODB_DATABASE)?;
    let client = mongodb::Client::with_uri_str(std::env::var(cmaeg::CMAEG_MONGODB_URI)?).await?;
    let count =
        cmaeg::records::import::import(&client.database(&database), std::path::Path::new(path))
            .await?;
    println!("Imported and verified {count} sealed records");
    Ok(())
}
