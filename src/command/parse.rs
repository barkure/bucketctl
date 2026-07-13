use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::session::expand_tilde;

use super::resolve::{parse_ls_target, parse_remote_spec};
use super::types::{Command, ExecMode};

pub fn parse_command_line(line: &str, mode: ExecMode) -> Result<Option<Command>> {
    if let Some(command) = line.strip_prefix('!') {
        return Ok(Some(Command::Shell {
            command: command.to_owned(),
        }));
    }

    let parts = shlex::split(line).ok_or_else(|| anyhow!("failed to parse command line"))?;
    parse_command_parts(&parts, mode)
}

pub fn parse_command_argv(args: &[String], mode: ExecMode) -> Result<Option<Command>> {
    if args.is_empty() {
        return Ok(None);
    }
    if let Some(command) = args[0].strip_prefix('!') {
        return Ok(Some(Command::Shell {
            command: command.to_owned(),
        }));
    }
    parse_command_parts(args, mode)
}

fn parse_command_parts(parts: &[String], mode: ExecMode) -> Result<Option<Command>> {
    if parts.is_empty() {
        return Ok(None);
    }

    match parts[0].as_str() {
        "help" => Ok(Some(Command::Help)),
        "exit" => {
            if mode == ExecMode::NonInteractive {
                bail!("`exit` is only available in the interactive shell");
            }
            Ok(Some(Command::Exit))
        }
        "pwd" => {
            if mode == ExecMode::NonInteractive {
                bail!("`pwd` is only available in the interactive shell");
            }
            Ok(Some(Command::Pwd))
        }
        "ls" => {
            let target = parts
                .get(1)
                .map(|value| parse_ls_target(value, mode))
                .transpose()?;
            Ok(Some(Command::Ls { target }))
        }
        "cd" => {
            if mode == ExecMode::NonInteractive {
                bail!("`cd` is only available in the interactive shell");
            }
            let target = parts.get(1).map(String::as_str).unwrap_or("/").to_owned();
            Ok(Some(Command::Cd { target }))
        }
        "mkdir" => {
            if parts.len() < 2 {
                bail!("usage: mkdir <remote-dir>");
            }
            Ok(Some(Command::Mkdir {
                target: parse_remote_spec(&parts[1], mode)?,
            }))
        }
        "put" => parse_put(parts, mode),
        "get" => parse_get(parts, mode),
        "rm" => parse_rm(parts, mode),
        other => bail!("unknown command `{other}`"),
    }
}

fn parse_put(parts: &[String], mode: ExecMode) -> Result<Option<Command>> {
    let (recursive, idx) = parse_recursive_flag(parts, 1);
    if parts.len() <= idx {
        bail!("usage: put [-r] <local> [remote]");
    }
    if mode == ExecMode::NonInteractive && parts.len() > idx + 2 {
        bail!("usage: put [-r] <local> [<profile>:/path]");
    }
    let local = PathBuf::from(expand_tilde(&parts[idx]));
    let remote = parts
        .get(idx + 1)
        .map(|value| parse_remote_spec(value, mode))
        .transpose()?;
    Ok(Some(Command::Put {
        local,
        remote,
        recursive,
    }))
}

fn parse_get(parts: &[String], mode: ExecMode) -> Result<Option<Command>> {
    let (recursive, idx) = parse_recursive_flag(parts, 1);
    if parts.len() <= idx {
        bail!("usage: get [-r] <remote> [local]");
    }
    if mode == ExecMode::NonInteractive && parts.len() > idx + 2 {
        bail!("usage: get [-r] <remote> [local]");
    }
    let remote = parse_remote_spec(&parts[idx], mode)?;
    let local = parts
        .get(idx + 1)
        .map(|value| PathBuf::from(expand_tilde(value)));
    Ok(Some(Command::Get {
        remote,
        local,
        recursive,
    }))
}

fn parse_recursive_flag(parts: &[String], start: usize) -> (bool, usize) {
    if parts.get(start).is_some_and(|arg| arg == "-r") {
        (true, start + 1)
    } else {
        (false, start)
    }
}

fn parse_rm(parts: &[String], mode: ExecMode) -> Result<Option<Command>> {
    let mut recursive = false;
    let mut yes = false;
    let mut remote = None;
    for arg in parts.iter().skip(1) {
        match arg.as_str() {
            "-r" => recursive = true,
            "-y" | "--yes" => yes = true,
            value => {
                if remote.is_some() {
                    bail!("usage: rm [-r] [-y] <remote>");
                }
                remote = Some(parse_remote_spec(value, mode)?);
            }
        }
    }
    let remote = remote.ok_or_else(|| {
        if recursive {
            anyhow!("usage: rm [-r] [-y] <remote-dir>")
        } else {
            anyhow!("usage: rm [-r] [-y] <remote>")
        }
    })?;
    let _ = mode;
    Ok(Some(Command::Rm {
        remote,
        recursive,
        yes,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::types::RemoteSpec;
    use super::*;

    #[test]
    fn argv_put_preserves_spaces_in_local_path() {
        let args = vec![
            "put".to_owned(),
            "./my file.txt".to_owned(),
            "mybucket:/key".to_owned(),
        ];
        let cmd = parse_command_argv(&args, ExecMode::NonInteractive)
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            Command::Put {
                local: PathBuf::from("./my file.txt"),
                remote: Some(RemoteSpec::ProfilePath {
                    profile: "mybucket".to_owned(),
                    path: "/key".to_owned(),
                }),
                recursive: false,
            }
        );
    }

    #[test]
    fn argv_get_parses_remote_path_with_spaces() {
        let args = vec![
            "get".to_owned(),
            "mybucket:/path with space".to_owned(),
            "./out".to_owned(),
        ];
        let cmd = parse_command_argv(&args, ExecMode::NonInteractive)
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            Command::Get {
                remote: RemoteSpec::ProfilePath {
                    profile: "mybucket".to_owned(),
                    path: "/path with space".to_owned(),
                },
                local: Some(PathBuf::from("./out")),
                recursive: false,
            }
        );
    }

    #[test]
    fn repl_put_preserves_spaces_via_shlex() {
        let cmd = parse_command_line("put './my file.txt' remote/", ExecMode::Interactive)
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            Command::Put {
                local: PathBuf::from("./my file.txt"),
                remote: Some(RemoteSpec::Path("remote/".to_owned())),
                recursive: false,
            }
        );
    }

    #[test]
    fn repl_get_rejects_profile_reference() {
        assert!(parse_command_line("get profile:foo", ExecMode::Interactive).is_err());
    }

    #[test]
    fn argv_ls_bare_name_is_ambiguous_target() {
        let args = vec!["ls".to_owned(), "myprofile".to_owned()];
        let cmd = parse_command_argv(&args, ExecMode::NonInteractive)
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            Command::Ls {
                target: Some(RemoteSpec::BareLsTarget("myprofile".to_owned())),
            }
        );
    }

    #[test]
    fn argv_ls_absolute_path() {
        let args = vec!["ls".to_owned(), "/path".to_owned()];
        let cmd = parse_command_argv(&args, ExecMode::NonInteractive)
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            Command::Ls {
                target: Some(RemoteSpec::Path("/path".to_owned())),
            }
        );
    }

    #[test]
    fn argv_cd_rejected_noninteractive() {
        let args = vec!["cd".to_owned(), "/x".to_owned()];
        assert!(parse_command_argv(&args, ExecMode::NonInteractive).is_err());
    }

    #[test]
    fn argv_rm_parses_recursive_and_yes_flags_in_any_order() {
        let args = vec![
            "rm".to_owned(),
            "-y".to_owned(),
            "-r".to_owned(),
            "mybucket:/prefix".to_owned(),
        ];
        let cmd = parse_command_argv(&args, ExecMode::NonInteractive)
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            Command::Rm {
                remote: RemoteSpec::ProfilePath {
                    profile: "mybucket".to_owned(),
                    path: "/prefix".to_owned(),
                },
                recursive: true,
                yes: true,
            }
        );
    }
}
