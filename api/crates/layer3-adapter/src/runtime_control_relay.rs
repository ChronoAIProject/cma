//! Fixed non-writing stdio-to-loopback transport used to reach a runtime.
//! Its isolated interpreter and exact script bytes are also writer-fence evidence.

pub const PROGRAM: &str = "/usr/bin/python3";
pub const SCRIPT: &str = include_str!("runtime_control_relay.py");

pub fn command() -> [&'static str; 5] {
    [PROGRAM, "-I", "-u", "-c", SCRIPT]
}

pub fn matches_command_line(bytes: &[u8]) -> bool {
    let expected = command()
        .into_iter()
        .flat_map(|argument| {
            argument
                .as_bytes()
                .iter()
                .copied()
                .chain(std::iter::once(0))
        })
        .collect::<Vec<_>>();
    bytes == expected
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_exact_isolated_nonwriting_relay_command_is_allowed() {
        let encode = |args: &[&str]| {
            args.iter()
                .flat_map(|arg| arg.as_bytes().iter().copied().chain(std::iter::once(0)))
                .collect::<Vec<_>>()
        };
        assert!(matches_command_line(&encode(&command())));
        for args in [
            vec![PROGRAM, "-u", "-c", SCRIPT],
            vec![PROGRAM, "-I", "-u", "-c", "print('other program')"],
            vec!["python3", "-I", "-u", "-c", SCRIPT],
            vec![PROGRAM, "-I", "-u", "-c", SCRIPT, "extra"],
        ] {
            assert!(!matches_command_line(&encode(&args)));
        }
        let mut unterminated = encode(&command());
        unterminated.pop();
        assert!(!matches_command_line(&unterminated));
    }
}
