use std::ffi::OsString;

const USAGE: &str = "usage: cmaeg [--print-openapi]";

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
    if !arguments.is_empty() {
        usage();
    }
    serve();
}

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn serve() {
    cmaeg::init_logging();
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
