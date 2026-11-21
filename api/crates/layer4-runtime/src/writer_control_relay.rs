//! A fixed isolated control relay performs only stdio/socket I/O. Unknown
//! Python processes, altered argv and a different interpreter remain writers.
use std::io::Read;
use std::path::Path;

#[cfg(target_os = "linux")]
pub(crate) fn is_nonwriting_relay(pid: i32) -> Result<bool, String> {
    let process = std::path::PathBuf::from(format!("/proc/{pid}"));
    inspect(&process)
}

fn inspect(process: &Path) -> Result<bool, String> {
    let first = match read_command(&process.join("cmdline")) {
        Ok(command) => command,
        Err(error) if vanished(&error) => return Ok(true),
        Err(error) => return Err(format!("inspect potential control relay: {error}")),
    };
    if !layer3_adapter::runtime_control_relay::matches_command_line(&first) {
        return Ok(false);
    }
    let executable = match std::fs::read_link(process.join("exe")) {
        Ok(executable) => executable,
        Err(error) if vanished(&error) => return Ok(true),
        Err(error) => return Err(format!("inspect control relay executable: {error}")),
    };
    let expected = std::fs::canonicalize(layer3_adapter::runtime_control_relay::PROGRAM)
        .map_err(|error| format!("inspect pinned control relay interpreter: {error}"))?;
    if executable != expected {
        return Ok(false);
    }
    match read_command(&process.join("cmdline")) {
        Ok(last) => Ok(last == first),
        Err(error) if vanished(&error) => Ok(true),
        Err(error) => Err(format!("revalidate control relay command: {error}")),
    }
}

fn read_command(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(16 * 1024)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn vanished(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writer_proof_exempts_only_exact_relay_script_and_interpreter() {
        let root = std::env::temp_dir().join(format!(
            "cma-control-relay-proof-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&root).unwrap();
        let expected =
            std::fs::canonicalize(layer3_adapter::runtime_control_relay::PROGRAM).unwrap();
        std::os::unix::fs::symlink(&expected, root.join("exe")).unwrap();
        let encode = |args: &[&str]| {
            args.iter()
                .flat_map(|arg| arg.as_bytes().iter().copied().chain(std::iter::once(0)))
                .collect::<Vec<_>>()
        };
        std::fs::write(
            root.join("cmdline"),
            encode(&layer3_adapter::runtime_control_relay::command()),
        )
        .unwrap();
        assert!(inspect(&root).unwrap());
        std::fs::write(
            root.join("cmdline"),
            encode(&[
                "/usr/bin/python3",
                "-I",
                "-u",
                "-c",
                "print('not a control relay')",
            ]),
        )
        .unwrap();
        assert!(!inspect(&root).unwrap());
        std::fs::write(
            root.join("cmdline"),
            encode(&layer3_adapter::runtime_control_relay::command()),
        )
        .unwrap();
        std::fs::remove_file(root.join("exe")).unwrap();
        std::os::unix::fs::symlink("/unexpected/interpreter", root.join("exe")).unwrap();
        assert!(!inspect(&root).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
}
