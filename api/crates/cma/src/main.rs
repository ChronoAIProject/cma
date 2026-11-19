//! Release identity and pre-runtime allocator qualification for the CMA executable.

// Avoid libc allocator retention and contention across async worker threads.
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(test)]
#[path = "../../cmaeg/tests/support/global_allocator.rs"]
mod global_allocator_tests;

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--allocator-self-test")) {
        // Qualify this executable's allocator before Tokio starts threads or
        // any serving, configuration, metrics or database composition runs.
        if arguments.next().is_some() {
            eprintln!("usage: cma --allocator-self-test");
            std::process::exit(2);
        }
        if let Err(error) = cma_allocator_qualification::run() {
            eprintln!("allocator-self-test outcome=failed error={error}");
            std::process::exit(1);
        }
        return;
    }
    cma::run(option_env!("CHRONO_BUILD_SHA").unwrap_or("development"));
}
