pub fn is_command_token(token: &str) -> bool {
    matches!(
        token,
        "help" | "ls" | "mkdir" | "put" | "get" | "rm" | "completion" | "config"
    ) || token.starts_with('!')
}

#[cfg(test)]
mod tests {
    use super::is_command_token;

    #[test]
    fn recognizes_builtin_commands() {
        assert!(is_command_token("ls"));
        assert!(is_command_token("completion"));
        assert!(is_command_token("config"));
        assert!(is_command_token("!pwd"));
    }

    #[test]
    fn rejects_profile_names() {
        assert!(!is_command_token("my-bucket"));
        assert!(!is_command_token("cloudflare-r2"));
    }
}
