use super::ProfileError;

pub const MAX_COMMAND_BYTES: usize = 4_096;

pub fn validate_command(command: &str) -> Result<(), ProfileError> {
    if command.is_empty()
        || command.len() > MAX_COMMAND_BYTES
        || command.contains('\0')
        || (command.contains('\n') && !contains_complete_heredoc(command))
    {
        return Err(ProfileError::new("invalid_command", "commands"));
    }
    Ok(())
}

fn contains_complete_heredoc(command: &str) -> bool {
    let lines: Vec<_> = command.lines().collect();
    lines.iter().enumerate().any(|(line_index, line)| {
        let Some(marker_index) = line.find("<<") else {
            return false;
        };
        let mut declaration = line[marker_index + 2..].trim_start();
        let strip_tabs = declaration.starts_with('-');
        if strip_tabs {
            declaration = declaration[1..].trim_start();
        }
        let Some(raw_delimiter) = declaration.split_ascii_whitespace().next() else {
            return false;
        };
        let delimiter = raw_delimiter
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
            .or_else(|| {
                raw_delimiter
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
            })
            .unwrap_or(raw_delimiter);
        !delimiter.is_empty()
            && lines[line_index + 1..].iter().any(|line| {
                let candidate = if strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line
                };
                candidate == delimiter
            })
    })
}

pub fn validate_workspace_name(name: &str) -> Result<(), ProfileError> {
    if name.is_empty() || name.len() > 64 || name == "." || name.starts_with('.') {
        return Err(ProfileError::new("invalid_workspace_name", "name"));
    }
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ProfileError::new("invalid_workspace_name", "name"));
    }
    Ok(())
}

pub fn validate_commands(commands: &[String], maximum: usize) -> Result<(), ProfileError> {
    if commands.len() > maximum {
        return Err(ProfileError::new("too_many_commands", "commands"));
    }
    commands
        .iter()
        .try_for_each(|command| validate_command(command))
}
