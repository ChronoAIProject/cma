//! Trusted entry gate inside an already-contained execution cell. This does
//! not create or attest containment; the runner owns that separate prerequisite.
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{Read, Write},
    os::{fd::AsFd, unix::ffi::OsStrExt, unix::process::CommandExt},
    path::Path,
    process::Command,
};

const SWITCH: &str = "--resource-execution-bootstrap-v1";
const PREPARED: &[u8] = b"CMA_RESOURCE_PREPARED_V1\n";
const ENTER: &[u8] = b"CMA_RESOURCE_ENTER_V1\n";
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 32 * 1024;

/// Runs before the supervisor's executor, logging, configuration or recovery.
/// Only the trusted runner may connect this stdin to its final entry gate.
pub(crate) fn command() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(OsStr::new(SWITCH)) {
        return None;
    }
    let status = execute(arguments).err().unwrap_or(125);
    eprintln!("resource execution bootstrap refused");
    Some(status)
}

fn execute(mut arguments: impl Iterator<Item = OsString>) -> Result<(), i32> {
    if arguments.next().as_deref() != Some(OsStr::new("--")) {
        return Err(125);
    }
    let mut command = Vec::new();
    let mut bytes = 0usize;
    for argument in arguments {
        bytes = bytes.checked_add(argument.as_bytes().len()).ok_or(125)?;
        if command.len() >= MAX_ARGUMENTS || bytes > MAX_ARGUMENT_BYTES {
            return Err(125);
        }
        command.push(argument);
    }
    let program = command.first().ok_or(125)?;
    if !Path::new(program).is_absolute() {
        return Err(125);
    }

    // File reads exactly the token length. Stdin's buffered reader could eat
    // queued native input, which must remain on fd 0 across exec.
    let mut input = File::from(
        std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|_| 125)?,
    );
    let mut output = std::io::stdout().lock();
    output.write_all(PREPARED).map_err(|_| 125)?;
    output.flush().map_err(|_| 125)?;
    drop(output);

    let mut release = [0; ENTER.len()];
    input.read_exact(&mut release).map_err(|_| 125)?;
    if release != ENTER {
        return Err(125);
    }
    drop(input);

    // Environment/cwd setup, native loading and startup hooks all occur after
    // explicit entry. The runner supplies any native environment in its command.
    let _error = Command::new(program)
        .args(&command[1..])
        .env_clear()
        .current_dir("/")
        .exec();
    Err(126)
}
