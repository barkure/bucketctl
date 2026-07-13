use anyhow::{Result, anyhow, bail};

use super::types::{ExecMode, RemoteSpec};

pub fn parse_remote_target(input: &str) -> Result<(String, String)> {
    let (profile, remote) = input
        .split_once(':')
        .ok_or_else(|| anyhow!("expected remote target like `<profile>:/path`"))?;
    if profile.is_empty() {
        bail!("invalid profile reference `{input}`");
    }
    let remote =
        normalize_remote_spec(remote).ok_or_else(|| anyhow!("missing remote path in `{input}`"))?;
    Ok((profile.to_owned(), remote))
}

pub fn normalize_remote_spec(remote: &str) -> Option<String> {
    let trimmed = remote.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub fn parse_remote_spec(input: &str, mode: ExecMode) -> Result<RemoteSpec> {
    if input.contains(':') {
        if mode == ExecMode::Interactive {
            bail!(
                "profile references like `profile:/path` are not supported in the interactive shell"
            );
        }
        let (profile, path) = parse_remote_target(input)?;
        return Ok(RemoteSpec::ProfilePath { profile, path });
    }
    Ok(RemoteSpec::Path(input.to_owned()))
}

pub fn parse_ls_target(input: &str, mode: ExecMode) -> Result<RemoteSpec> {
    if input.contains(':') {
        if mode == ExecMode::Interactive {
            bail!(
                "profile references like `profile:/path` are not supported in the interactive shell"
            );
        }
        let (profile, path) = parse_remote_target(input)?;
        return Ok(RemoteSpec::ProfilePath { profile, path });
    }
    if mode == ExecMode::NonInteractive && !input.starts_with('/') {
        Ok(RemoteSpec::BareLsTarget(input.to_owned()))
    } else {
        Ok(RemoteSpec::Path(input.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_target_splits_profile_and_path() {
        let (profile, remote) = parse_remote_target("mybucket:/path/to").unwrap();
        assert_eq!(profile, "mybucket");
        assert_eq!(remote, "/path/to");
    }

    #[test]
    fn parse_remote_target_rejects_missing_profile() {
        assert!(parse_remote_target(":/path").is_err());
    }

    #[test]
    fn normalize_remote_spec_trims() {
        assert_eq!(normalize_remote_spec("  /x  ").as_deref(), Some("/x"));
        assert!(normalize_remote_spec("").is_none());
    }

    #[test]
    fn parse_remote_spec_rejects_profile_in_interactive_mode() {
        assert!(parse_remote_spec("mybucket:/path", ExecMode::Interactive).is_err());
    }
}
