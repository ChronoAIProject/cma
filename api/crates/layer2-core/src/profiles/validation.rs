use super::ProfileError;

pub const MAX_COMMAND_BYTES: usize = 4_096;

pub fn validate_command(command: &str) -> Result<(), ProfileError> {
    if command.is_empty() || command.len() > MAX_COMMAND_BYTES || command.contains('\0') {
        return Err(ProfileError::new("invalid_command", "commands"));
    }
    Ok(())
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
