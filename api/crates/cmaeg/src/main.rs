use std::ffi::OsString;

const USAGE: &str = "usage: cmaeg [--print-openapi|--migrate|--print-runtime-role|--print-migration-role|--import-legacy-records PATH]";

fn main() {
    // `args_os` never panics: `args` aborts with exit 101 on a non-UTF-8
    // argument before any usage handling could run.
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    // Modes that need no runtime are matched on the raw arguments first, so
    // they run before one exists.
    if arguments == ["--print-openapi"] {
        print!("{}", cmaeg::openapi::snapshot());
        return;
    }
    let Some(arguments) = arguments
        .iter()
        .map(|argument| argument.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        usage();
    };
    if matches!(
        arguments.first().copied(),
        Some("--migrate" | "--print-runtime-role" | "--print-migration-role")
    ) && arguments.len() == 1
    {
        run_database_command(arguments[0]);
        return;
    }
    if arguments.first().copied() == Some("--import-legacy-records") && arguments.len() == 2 {
        run_import(arguments[1]);
        return;
    }
    serve();
}

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn serve() {
    cmaeg::init_logging();
    cmaeg::install_panic_hook();
    // Both failures exit through `process::exit`: threads a failed prewarm
    // left parked on its barrier end with the process, before any listener binds.
    let runtime = match cmaeg::runtime::build() {
        Ok(runtime) => runtime,
        Err(cmaeg::runtime::RuntimeError::Prewarm) => {
            tracing::error!("CMAEG blocking pool prewarm failed");
            std::process::exit(1);
        }
        Err(error) => {
            tracing::error!(%error, "CMAEG runtime unavailable");
            std::process::exit(1);
        }
    };
    if let Err(error) = runtime.block_on(cmaeg::run()) {
        tracing::error!(%error, "CMAEG stopped");
        std::process::exit(1);
    }
}

fn run_database_command(command: &str) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("cannot initialize command runtime: {error}");
            std::process::exit(1);
        });
    if let Err(error) = runtime.block_on(async {
        let database = std::env::var(cmaeg::CMAEG_MONGODB_DATABASE)?;
        if command != "--migrate" {
            println!(
                "{}",
                cmaeg::records::mongo::role(&database, command == "--print-migration-role")
            );
            return Ok::<(), Box<dyn std::error::Error>>(());
        }
        let uri = std::env::var(cmaeg::CMAEG_MONGODB_URI)?;
        let client = mongodb::Client::with_uri_str(uri).await?;
        cmaeg::records::mongo::migrate(&client.database(&database)).await?;
        Ok(())
    }) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_import(path: &str) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("cannot initialize command runtime: {error}");
            std::process::exit(1);
        });
    if let Err(error) = runtime.block_on(async {
        let database = std::env::var(cmaeg::CMAEG_MONGODB_DATABASE)?;
        let uri = std::env::var(cmaeg::CMAEG_MONGODB_URI)?;
        let client = mongodb::Client::with_uri_str(uri).await?;
        let count =
            cmaeg::records::import::import(&client.database(&database), std::path::Path::new(path))
                .await?;
        println!("Imported and verified {count} sealed records");
        Ok::<(), Box<dyn std::error::Error>>(())
    }) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
