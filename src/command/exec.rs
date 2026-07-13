use std::{
    env, io,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    process::Command as OsCommand,
    sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow, bail};
use tokio::runtime::Runtime;

use crate::{
    s3::S3Backend,
    session::{Session, attach_profile_for_command, with_session, with_session_mut},
    ui,
};

use super::print::{list_and_print, print_help, print_remote_entries};
use super::transfer::{
    execute_get_recursive, execute_put_recursive, finish_recursive, resolve_get_recursive_layout,
    resolve_put_recursive_prefix,
};
use super::types::{Command, ExecMode, ExecOutcome, RemoteSpec};

pub fn execute(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    command: Command,
) -> Result<ExecOutcome> {
    match command {
        Command::Shell { command } => {
            run_local_shell(&command)?;
            Ok(ExecOutcome::Continue)
        }
        Command::Help => {
            print_help(mode);
            Ok(ExecOutcome::Continue)
        }
        Command::Exit => Ok(ExecOutcome::ExitRepl),
        Command::Pwd => {
            let location = with_session(session, |sess| sess.location_display())?;
            println!("{location}");
            Ok(ExecOutcome::Continue)
        }
        Command::Ls { target } => execute_ls(runtime, session, mode, target),
        Command::Cd { target } => execute_cd(runtime, session, &target),
        Command::Mkdir { target } => execute_mkdir(runtime, session, mode, &target),
        Command::Put {
            local,
            remote,
            recursive,
        } => execute_put(runtime, session, mode, &local, remote.as_ref(), recursive),
        Command::Get {
            remote,
            local,
            recursive,
        } => execute_get(runtime, session, mode, &remote, local.as_ref(), recursive),
        Command::Rm {
            remote,
            recursive,
            yes,
        } => execute_rm(runtime, session, mode, &remote, recursive, yes),
    }
}

fn execute_ls(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    target: Option<RemoteSpec>,
) -> Result<ExecOutcome> {
    match target {
        None => {
            if mode == ExecMode::NonInteractive {
                if let Some(default) = with_session(session, |sess| {
                    sess.default_profile_name().map(ToOwned::to_owned)
                })? {
                    attach_profile_for_command(runtime, session, &default)?;
                    let (bucket, prefix, s3) = ls_context(session, ".")?;
                    list_and_print(runtime, &s3, &bucket, &prefix)?;
                } else {
                    let profiles = with_session(session, |sess| sess.list_profiles())?;
                    if !profiles.is_empty() {
                        let rendered = profiles
                            .into_iter()
                            .map(|profile| ui::stdout_profile(&profile))
                            .collect::<Vec<_>>()
                            .join("  ");
                        println!("{rendered}");
                    }
                }
            } else {
                let (bucket, prefix, s3) = ls_context(session, ".")?;
                let entries = runtime.block_on(s3.list_prefix(&bucket, &prefix))?;
                print_remote_entries(&entries);
            }
        }
        Some(spec) => {
            let (bucket, prefix, s3) = resolve_ls_context(runtime, session, mode, &spec)?;
            list_and_print(runtime, &s3, &bucket, &prefix)?;
        }
    }
    Ok(ExecOutcome::Continue)
}

fn resolve_ls_context(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    spec: &RemoteSpec,
) -> Result<(String, String, S3Backend)> {
    match spec {
        RemoteSpec::ProfilePath { profile, path } => {
            attach_profile_for_command(runtime, session, profile)?;
            ls_context(session, path)
        }
        RemoteSpec::BareLsTarget(name) => {
            let is_profile = with_session(session, |sess| sess.has_profile(name))?;
            if is_profile {
                attach_profile_for_command(runtime, session, name)?;
                ls_context(session, ".")
            } else if let Some(default) = with_session(session, |sess| {
                sess.default_profile_name().map(ToOwned::to_owned)
            })? {
                attach_profile_for_command(runtime, session, &default)?;
                ls_context(session, name)
            } else {
                bail!("profile `{name}` not found, and no default profile set");
            }
        }
        RemoteSpec::Path(path) => {
            if mode == ExecMode::NonInteractive
                && with_session(session, |sess| sess.current_bucket().is_none())?
                && let Some(default) = with_session(session, |sess| {
                    sess.default_profile_name().map(ToOwned::to_owned)
                })?
            {
                attach_profile_for_command(runtime, session, &default)?;
            }
            ls_context(session, path)
        }
    }
}

fn ls_context(session: &Arc<Mutex<Session>>, path: &str) -> Result<(String, String, S3Backend)> {
    with_session(session, |sess| -> Result<_> {
        let bucket = sess.selected_bucket()?.to_owned();
        let prefix = sess.resolve_remote(path)?;
        Ok((bucket, prefix, sess.selected_s3()?.clone()))
    })?
}

fn execute_cd(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    target: &str,
) -> Result<ExecOutcome> {
    let (bucket, resolved, needs_check, s3) = with_session(session, |sess| -> Result<_> {
        let resolved = sess.resolve_remote(target)?;
        Ok((
            sess.selected_bucket()?.to_owned(),
            resolved.clone(),
            !resolved.is_empty(),
            sess.selected_s3()?.clone(),
        ))
    })??;

    if needs_check && !runtime.block_on(s3.remote_dir_exists(&bucket, &resolved))? {
        let display = if resolved.is_empty() {
            "/".to_owned()
        } else {
            format!("/{}", resolved.trim_end_matches('/'))
        };
        bail!("no such remote directory: {display}");
    }

    let new_dir = with_session_mut(session, |sess| sess.change_dir(target))??;
    println!("{new_dir}");
    Ok(ExecOutcome::Continue)
}

fn execute_mkdir(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    target: &RemoteSpec,
) -> Result<ExecOutcome> {
    let (bucket, key, s3) = resolve_remote_operation(runtime, session, mode, target)?;
    runtime.block_on(s3.create_dir(&bucket, &key))?;
    Ok(ExecOutcome::Continue)
}

fn execute_put(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    local: &Path,
    remote: Option<&RemoteSpec>,
    recursive: bool,
) -> Result<ExecOutcome> {
    if recursive {
        return execute_put_recursive_paths(runtime, session, mode, local, remote);
    }

    let (bucket, remote_path, s3) = match remote {
        Some(spec) => {
            let (bucket, s3) = resolve_profile_context(runtime, session, mode, spec)?;
            let remote_path = remote_path_from_spec(spec);
            (bucket, remote_path, s3)
        }
        None => {
            let bucket = with_session(session, |sess| {
                sess.selected_bucket().map(ToOwned::to_owned)
            })??;
            let s3 = with_session(session, |sess| sess.selected_s3().cloned())??;
            (bucket, None, s3)
        }
    };

    let key = match remote_path.as_deref() {
        Some(remote)
            if !remote.ends_with('/')
                && runtime.block_on(s3.remote_dir_exists(&bucket, remote))? =>
        {
            with_session(session, |sess| {
                sess.resolve_upload_target_in_dir(local, remote)
            })??
        }
        _ => with_session(session, |sess| {
            sess.resolve_upload_target(local, remote_path.as_deref())
        })??,
    };

    finish_transfer(mode, runtime.block_on(s3.put_file(&bucket, &key, local)))
}

fn execute_put_recursive_paths(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    local: &Path,
    remote: Option<&RemoteSpec>,
) -> Result<ExecOutcome> {
    let (bucket, remote_path, s3) = match remote {
        Some(spec) => {
            let (bucket, s3) = resolve_profile_context(runtime, session, mode, spec)?;
            let remote_path = remote_path_from_spec(spec);
            (bucket, remote_path, s3)
        }
        None => {
            let bucket = with_session(session, |sess| {
                sess.selected_bucket().map(ToOwned::to_owned)
            })??;
            let s3 = with_session(session, |sess| sess.selected_s3().cloned())??;
            (bucket, None, s3)
        }
    };

    let remote_is_existing_dir = match remote_path.as_deref() {
        Some(remote)
            if !remote.ends_with('/') && !remote.eq_ignore_ascii_case(".") && remote != "/" =>
        {
            runtime.block_on(s3.remote_dir_exists(&bucket, remote))?
        }
        _ => false,
    };

    let key_prefix = with_session(session, |sess| {
        resolve_put_recursive_prefix(sess, local, remote_path.as_deref(), remote_is_existing_dir)
    })??;

    let (report, interrupted) =
        execute_put_recursive(runtime, &s3, &bucket, local, &key_prefix, mode)?;
    finish_recursive(mode, report, interrupted)
}

fn execute_get(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    remote: &RemoteSpec,
    local: Option<&PathBuf>,
    recursive: bool,
) -> Result<ExecOutcome> {
    if recursive {
        return execute_get_recursive_paths(runtime, session, mode, remote, local);
    }

    let remote_path = remote_path_from_spec(remote).ok_or_else(|| {
        anyhow!("expected remote path like `<profile>:/path` or a cwd-relative path")
    })?;
    let (bucket, key, s3) = resolve_remote_operation(runtime, session, mode, remote)?;
    let local_arg = local.map(|path| path.to_string_lossy().into_owned());
    let local = Session::resolve_download_target(&remote_path, local_arg.as_deref())?;
    finish_transfer(mode, runtime.block_on(s3.get_file(&bucket, &key, &local)))
}

fn execute_get_recursive_paths(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    remote: &RemoteSpec,
    local: Option<&PathBuf>,
) -> Result<ExecOutcome> {
    let (bucket, remote_prefix, s3) = resolve_remote_operation(runtime, session, mode, remote)?;
    let layout = resolve_get_recursive_layout(&remote_prefix, local.map(PathBuf::as_path))?;
    let (report, interrupted) =
        execute_get_recursive(runtime, &s3, &bucket, &remote_prefix, &layout, mode)?;
    finish_recursive(mode, report, interrupted)
}

fn execute_rm(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    remote: &RemoteSpec,
    recursive: bool,
    yes: bool,
) -> Result<ExecOutcome> {
    let (bucket, key, s3) = resolve_remote_operation(runtime, session, mode, remote)?;
    if recursive {
        let keys = runtime.block_on(s3.list_keys_for_delete(&bucket, &key))?;
        if keys.is_empty() {
            println!("nothing to delete");
            return Ok(ExecOutcome::Continue);
        }
        if !yes {
            match mode {
                ExecMode::NonInteractive => {
                    bail!("recursive delete in non-interactive mode requires -y");
                }
                ExecMode::Interactive if !confirm_recursive_delete(keys.len(), &key)? => {
                    return Ok(ExecOutcome::Continue);
                }
                ExecMode::Interactive => {}
            }
        }
        let deleted = runtime.block_on(s3.delete_keys(&bucket, &keys))?;
        println!("deleted {deleted} object(s)");
    } else {
        runtime.block_on(s3.delete_object(&bucket, &key))?;
    }
    Ok(ExecOutcome::Continue)
}

fn confirm_recursive_delete(count: usize, prefix: &str) -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!("recursive delete requires -y when stdin is not a terminal");
    }
    let display = if prefix.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", prefix.trim_end_matches('/'))
    };
    print!("delete {count} object(s) under {display}? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn resolve_remote_operation(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    spec: &RemoteSpec,
) -> Result<(String, String, S3Backend)> {
    let remote_path = remote_path_from_spec(spec).ok_or_else(|| {
        anyhow!("expected remote path like `<profile>:/path` or a cwd-relative path")
    })?;
    let (bucket, s3) = resolve_profile_context(runtime, session, mode, spec)?;
    let key = with_session(session, |sess| sess.resolve_remote(&remote_path))??;
    Ok((bucket, key, s3))
}

fn resolve_profile_context(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    spec: &RemoteSpec,
) -> Result<(String, S3Backend)> {
    match spec {
        RemoteSpec::ProfilePath { profile, .. } => {
            attach_profile_for_command(runtime, session, profile)?;
        }
        RemoteSpec::BareLsTarget(_) => {
            bail!("expected a remote path, not a profile root");
        }
        RemoteSpec::Path(_) => {
            if mode == ExecMode::NonInteractive
                && with_session(session, |sess| sess.current_bucket().is_none())?
            {
                bail!("no default profile attached; use `<profile>:/path`");
            }
        }
    }
    with_session(session, |sess| -> Result<_> {
        Ok((
            sess.selected_bucket()?.to_owned(),
            sess.selected_s3()?.clone(),
        ))
    })?
}

fn remote_path_from_spec(spec: &RemoteSpec) -> Option<String> {
    match spec {
        RemoteSpec::Path(path) | RemoteSpec::ProfilePath { path, .. } => Some(path.clone()),
        RemoteSpec::BareLsTarget(_) => None,
    }
}

fn finish_transfer(mode: ExecMode, result: Result<()>) -> Result<ExecOutcome> {
    if let Err(err) = result {
        if is_interrupted(&err) {
            return match mode {
                ExecMode::Interactive => Ok(ExecOutcome::Continue),
                ExecMode::NonInteractive => Ok(ExecOutcome::ExitCode(130)),
            };
        }
        return Err(err);
    }
    Ok(ExecOutcome::Continue)
}

fn is_interrupted(err: &anyhow::Error) -> bool {
    err.downcast_ref::<io::Error>()
        .is_some_and(|inner| inner.kind() == io::ErrorKind::Interrupted)
}

fn run_local_shell(command: &str) -> Result<()> {
    if command.trim().is_empty() {
        return Ok(());
    }

    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let shell_name = Path::new(&shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let mut process = OsCommand::new(&shell);
    match shell_name {
        "zsh" | "bash" => {
            process
                .arg("-lc")
                .arg(build_shell_command(shell_name))
                .arg(shell_name)
                .arg(command);
        }
        _ => {
            process.arg("-c").arg(command);
        }
    }

    let status = process.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("local command exited with status {status}"))
    }
}

fn build_shell_command(shell_name: &str) -> String {
    match shell_name {
        "zsh" => [
            "setopt aliases",
            "source ~/.zshrc >/dev/null 2>&1 || true",
            "eval \"$1\"",
        ]
        .join("\n"),
        "bash" => [
            "shopt -s expand_aliases",
            "source ~/.bashrc >/dev/null 2>&1 || true",
            "eval \"$1\"",
        ]
        .join("\n"),
        _ => "eval \"$1\"".to_owned(),
    }
}
