use std::{
    env, io,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow, bail};
use tokio::runtime::Runtime;

use crate::{
    s3::S3Backend,
    session::Session,
    ui,
};

pub(crate) fn run_command(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    line: &str,
) -> Result<bool> {
    if let Some(command) = line.strip_prefix('!') {
        return run_local_shell(command).map(|_| false);
    }

    let parts = shlex::split(line).ok_or_else(|| anyhow!("failed to parse command line"))?;
    if parts.is_empty() {
        return Ok(false);
    }

    match parts[0].as_str() {
        "help" => {
            print_help();
            Ok(false)
        }
        "exit" => Ok(true),
        "pwd" => {
            let location = with_session(session, |sess| sess.location_display())?;
            println!("{location}");
            Ok(false)
        }
        "ls" => {
            let target = parts.get(1).cloned();
            let (bucket, prefix, s3) = with_session(session, |sess| -> Result<_> {
                let bucket = sess.selected_bucket()?.to_owned();
                let prefix = sess.resolve_remote(target.as_deref().unwrap_or("."))?;
                Ok((bucket, prefix, sess.selected_s3()?.clone()))
            })??;
            for entry in runtime.block_on(s3.list_prefix(&bucket, &prefix))? {
                if entry.is_dir {
                    if let Some(modified) = entry.modified.as_deref() {
                        println!(
                            "{}  {}  {}",
                            ui::stdout_dir_label("DIR"),
                            ui::stdout_time(modified),
                            ui::stdout_dir(&format!("{}/", entry.name))
                        );
                    } else {
                        println!(
                            "{}  {}  {}",
                            ui::stdout_dir_label("DIR"),
                            ui::stdout_time(""),
                            ui::stdout_dir(&format!("{}/", entry.name))
                        );
                    }
                } else if let Some(size) = entry.size {
                    let size = ui::stdout_size(&ui::format_bytes(size.max(0) as u64));
                    if let Some(modified) = entry.modified.as_deref() {
                        println!(
                            "{}  {}  {}",
                            size,
                            ui::stdout_time(modified),
                            ui::stdout_file(&entry.name)
                        );
                    } else {
                        println!("{}  {}", size, ui::stdout_file(&entry.name));
                    }
                } else {
                    println!("{}", ui::stdout_file(&entry.name));
                }
            }
            Ok(false)
        }
        "cd" => {
            let target = parts.get(1).map(String::as_str).unwrap_or("/");
            let new_dir = with_session_mut(session, |sess| sess.change_dir(target))??;
            println!("{new_dir}");
            Ok(false)
        }
        "mkdir" => {
            if parts.len() < 2 {
                bail!("usage: mkdir <remote-dir>");
            }
            let remote = parts[1].clone();
            let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                let bucket = sess.selected_bucket()?.to_owned();
                let key = sess.resolve_remote(&remote)?;
                Ok((bucket, key, sess.selected_s3()?.clone()))
            })??;
            runtime.block_on(s3.create_dir(&bucket, &key))?;
            Ok(false)
        }
        "put" => {
            if parts.len() < 2 {
                bail!("usage: put <local> [remote]");
            }
            let local = PathBuf::from(expand_tilde(&parts[1]));
            let (bucket, remote, s3) = with_session(session, |sess| -> Result<_> {
                let bucket = sess.selected_bucket()?.to_owned();
                Ok((
                    bucket,
                    parts.get(2).map(String::to_owned),
                    sess.selected_s3()?.clone(),
                ))
            })??;
            let key = match remote.as_deref() {
                Some(remote) if !remote.ends_with('/') && runtime.block_on(s3.remote_dir_exists(&bucket, remote))? => {
                    with_session(session, |sess| sess.resolve_upload_target_in_dir(&local, remote))??
                }
                _ => with_session(session, |sess| {
                    sess.resolve_upload_target(&local, remote.as_deref())
                })??,
            };
            if let Err(err) = runtime.block_on(s3.put_file(&bucket, &key, &local)) {
                if is_interrupted(&err) {
                    return Ok(false);
                }
                return Err(err);
            }
            Ok(false)
        }
        "get" => {
            if parts.len() < 2 {
                bail!("usage: get <remote> [local]");
            }
            let remote = parts[1].clone();
            let local =
                Session::resolve_download_target(&remote, parts.get(2).map(String::as_str))?;
            let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                let bucket = sess.selected_bucket()?.to_owned();
                let key = sess.resolve_remote(&remote)?;
                Ok((bucket, key, sess.selected_s3()?.clone()))
            })??;
            if let Err(err) = runtime.block_on(s3.get_file(&bucket, &key, &local)) {
                if is_interrupted(&err) {
                    return Ok(false);
                }
                return Err(err);
            }
            Ok(false)
        }
        "rm" => {
            if parts.len() < 2 {
                bail!("usage: rm <remote> | rm -r <remote-dir>");
            }
            let recursive = parts.get(1).is_some_and(|arg| arg == "-r");
            let remote_index = if recursive { 2 } else { 1 };
            let remote = parts
                .get(remote_index)
                .cloned()
                .ok_or_else(|| anyhow!("usage: rm <remote> | rm -r <remote-dir>"))?;
            let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                let bucket = sess.selected_bucket()?.to_owned();
                let key = sess.resolve_remote(&remote)?;
                Ok((bucket, key, sess.selected_s3()?.clone()))
            })??;
            if recursive {
                let deleted = runtime.block_on(s3.delete_prefix_recursive(&bucket, &key))?;
                println!("deleted {deleted} object(s)");
            } else {
                runtime.block_on(s3.delete_object(&bucket, &key))?;
            }
            Ok(false)
        }
        other => bail!("unknown command `{other}`"),
    }
}

pub(crate) fn run_noninteractive_command_line(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    line: &str,
) -> Result<bool> {
    if let Some(command) = line.strip_prefix('!') {
        return run_local_shell(command).map(|_| false);
    }

    let parts = shlex::split(line).ok_or_else(|| anyhow!("failed to parse command line"))?;
    if parts.is_empty() {
        return Ok(false);
    }

    match parts[0].as_str() {
        "help" => {
            print_noninteractive_help();
            Ok(false)
        }
        "ls" => {
            if let Some(target) = parts.get(1) {
                if target.contains(':') {
                    let (profile, remote) = parse_remote_target(target)?;
                    attach_profile_for_command(runtime, session, &profile)?;
                    let bucket =
                        with_session(session, |sess| sess.selected_bucket().map(ToOwned::to_owned))??;
                    let s3 = with_session(session, |sess| sess.selected_s3().cloned())??;
                    let prefix = with_session(session, |sess| -> Result<_> {
                        sess.resolve_remote(&remote)
                    })??;
                    list_and_print(runtime, &s3, &bucket, &prefix)?;
                } else {
                    let is_profile = with_session(session, |sess| sess.has_profile(target))?;
                    if is_profile {
                        attach_profile_for_command(runtime, session, target)?;
                        let bucket =
                            with_session(session, |sess| sess.selected_bucket().map(ToOwned::to_owned))??;
                        let s3 = with_session(session, |sess| sess.selected_s3().cloned())??;
                        let prefix = with_session(session, |sess| sess.resolve_remote("."))??;
                        list_and_print(runtime, &s3, &bucket, &prefix)?;
                    } else if let Some(default) =
                        with_session(session, |sess| sess.default_profile_name().map(ToOwned::to_owned))?
                    {
                        attach_profile_for_command(runtime, session, &default)?;
                        let bucket =
                            with_session(session, |sess| sess.selected_bucket().map(ToOwned::to_owned))??;
                        let s3 = with_session(session, |sess| sess.selected_s3().cloned())??;
                        let prefix = with_session(session, |sess| -> Result<_> {
                            sess.resolve_remote(target)
                        })??;
                        list_and_print(runtime, &s3, &bucket, &prefix)?;
                    } else {
                        bail!("profile `{target}` not found, and no default profile set");
                    }
                }
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
            Ok(false)
        }
        "put" => {
            if parts.len() < 2 || parts.len() > 3 {
                bail!("usage: put <local> [<profile>:/path]");
            }
            let local = PathBuf::from(expand_tilde(&parts[1]));
            if let Some(target) = parts.get(2) {
                if target.contains(':') {
                    let (profile, remote) = parse_remote_target(target)?;
                    attach_profile_for_command(runtime, session, &profile)?;
                    let (bucket, s3) = with_session(session, |sess| -> Result<_> {
                        let bucket = sess.selected_bucket()?.to_owned();
                        Ok((bucket, sess.selected_s3()?.clone()))
                    })??;
                    let key =
                        if !remote.ends_with('/') && runtime.block_on(s3.remote_dir_exists(&bucket, &remote))? {
                            with_session(session, |sess| sess.resolve_upload_target_in_dir(&local, &remote))??
                        } else {
                            with_session(session, |sess| sess.resolve_upload_target(&local, Some(&remote)))??
                        };
                    runtime.block_on(s3.put_file(&bucket, &key, &local))?;
                } else {
                    let (bucket, s3) = with_session(session, |sess| -> Result<_> {
                        let bucket = sess.selected_bucket()?.to_owned();
                        Ok((bucket, sess.selected_s3()?.clone()))
                    })??;
                    let key =
                        if !target.ends_with('/') && runtime.block_on(s3.remote_dir_exists(&bucket, target))? {
                            with_session(session, |sess| sess.resolve_upload_target_in_dir(&local, target))??
                        } else {
                            with_session(session, |sess| sess.resolve_upload_target(&local, Some(target)))??
                        };
                    runtime.block_on(s3.put_file(&bucket, &key, &local))?;
                }
            } else {
                let (bucket, s3) = with_session(session, |sess| -> Result<_> {
                    let bucket = sess.selected_bucket()?.to_owned();
                    Ok((bucket, sess.selected_s3()?.clone()))
                })??;
                let key = with_session(session, |sess| sess.resolve_upload_target(&local, None))??;
                runtime.block_on(s3.put_file(&bucket, &key, &local))?;
            }
            Ok(false)
        }
        "get" => {
            if parts.len() < 2 || parts.len() > 3 {
                bail!("usage: get <remote> [local]");
            }
            let target = &parts[1];
            if target.contains(':') {
                let (profile, remote) = parse_remote_target(target)?;
                attach_profile_for_command(runtime, session, &profile)?;
                let local =
                    Session::resolve_download_target(&remote, parts.get(2).map(String::as_str))?;
                let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                    let bucket = sess.selected_bucket()?.to_owned();
                    let key = sess.resolve_remote(&remote)?;
                    Ok((bucket, key, sess.selected_s3()?.clone()))
                })??;
                runtime.block_on(s3.get_file(&bucket, &key, &local))?;
            } else {
                let remote = target.clone();
                let local =
                    Session::resolve_download_target(&remote, parts.get(2).map(String::as_str))?;
                let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                    let bucket = sess.selected_bucket()?.to_owned();
                    let key = sess.resolve_remote(&remote)?;
                    Ok((bucket, key, sess.selected_s3()?.clone()))
                })??;
                runtime.block_on(s3.get_file(&bucket, &key, &local))?;
            }
            Ok(false)
        }
        "mkdir" => {
            if parts.len() < 2 {
                bail!("usage: mkdir [<profile>:/path]");
            }
            let target = &parts[1];
            if target.contains(':') {
                let (profile, remote) = parse_remote_target(target)?;
                attach_profile_for_command(runtime, session, &profile)?;
                let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                    let bucket = sess.selected_bucket()?.to_owned();
                    let key = sess.resolve_remote(&remote)?;
                    Ok((bucket, key, sess.selected_s3()?.clone()))
                })??;
                runtime.block_on(s3.create_dir(&bucket, &key))?;
            } else {
                let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                    let bucket = sess.selected_bucket()?.to_owned();
                    let key = sess.resolve_remote(target)?;
                    Ok((bucket, key, sess.selected_s3()?.clone()))
                })??;
                runtime.block_on(s3.create_dir(&bucket, &key))?;
            }
            Ok(false)
        }
        "rm" => {
            if parts.len() < 2 {
                bail!("usage: rm <remote> | rm -r <remote-dir>");
            }
            let recursive = parts.get(1).is_some_and(|arg| arg == "-r");
            let remote_index = if recursive { 2 } else { 1 };
            let remote_spec = parts
                .get(remote_index)
                .ok_or_else(|| anyhow!("usage: rm <remote> | rm -r <remote-dir>"))?;
            if remote_spec.contains(':') {
                let (profile, remote) = parse_remote_target(remote_spec)?;
                attach_profile_for_command(runtime, session, &profile)?;
                let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                    let bucket = sess.selected_bucket()?.to_owned();
                    let key = sess.resolve_remote(&remote)?;
                    Ok((bucket, key, sess.selected_s3()?.clone()))
                })??;
                if recursive {
                    let deleted = runtime.block_on(s3.delete_prefix_recursive(&bucket, &key))?;
                    println!("deleted {deleted} object(s)");
                } else {
                    runtime.block_on(s3.delete_object(&bucket, &key))?;
                }
            } else {
                let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                    let bucket = sess.selected_bucket()?.to_owned();
                    let key = sess.resolve_remote(remote_spec)?;
                    Ok((bucket, key, sess.selected_s3()?.clone()))
                })??;
                if recursive {
                    let deleted = runtime.block_on(s3.delete_prefix_recursive(&bucket, &key))?;
                    println!("deleted {deleted} object(s)");
                } else {
                    runtime.block_on(s3.delete_object(&bucket, &key))?;
                }
            }
            Ok(false)
        }
        "pwd" => bail!("`pwd` is only available in the interactive shell"),
        "cd" => bail!("`cd` is only available in the interactive shell"),
        "exit" => bail!("`exit` is only available in the interactive shell"),
        other => bail!("unknown command `{other}`"),
    }
}

fn print_help() {
    println!("ls [path]");
    println!("cd [path]");
    println!("pwd");
    println!("mkdir <remote-dir>");
    println!("put <local> [remote]");
    println!("get <remote> [local]");
    println!("rm <remote>");
    println!("rm -r <remote-dir>");
    println!("!<command>");
    println!("Ctrl-D");
    println!("exit");
}

fn print_noninteractive_help() {
    println!("ls [<profile>[:/path]]");
    println!("mkdir <profile>:/path");
    println!("put <local> <profile>:/path");
    println!("get <profile>:/path [local]");
    println!("rm <profile>:/path");
    println!("rm -r <profile>:/path");
    println!("!<local command>");
}

fn list_and_print(
    runtime: &Arc<Runtime>,
    s3: &S3Backend,
    bucket: &str,
    prefix: &str,
) -> Result<()> {
    for entry in runtime.block_on(s3.list_prefix(bucket, prefix))? {
        if entry.is_dir {
            if let Some(modified) = entry.modified.as_deref() {
                println!(
                    "{}  {}  {}",
                    ui::stdout_dir_label("DIR"),
                    ui::stdout_time(modified),
                    ui::stdout_dir(&format!("{}/", entry.name))
                );
            } else {
                println!(
                    "{}  {}  {}",
                    ui::stdout_dir_label("DIR"),
                    ui::stdout_time(""),
                    ui::stdout_dir(&format!("{}/", entry.name))
                );
            }
        } else if let Some(size) = entry.size {
            let size = ui::stdout_size(&ui::format_bytes(size.max(0) as u64));
            if let Some(modified) = entry.modified.as_deref() {
                println!(
                    "{}  {}  {}",
                    size,
                    ui::stdout_time(modified),
                    ui::stdout_file(&entry.name)
                );
            } else {
                println!("{}  {}", size, ui::stdout_file(&entry.name));
            }
        } else {
            println!("{}", ui::stdout_file(&entry.name));
        }
    }
    Ok(())
}

pub(crate) fn attach_profile_for_command(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    profile_name: &str,
) -> Result<()> {
    let profile = with_session(session, |sess| sess.profile_config(profile_name).cloned())??;
    let s3 = runtime.block_on(S3Backend::connect(&profile))?;
    with_session_mut(session, |sess| {
        sess.attach_profile(profile_name.to_owned(), profile.bucket.clone(), s3)
    })?;
    Ok(())
}

fn parse_remote_target(input: &str) -> Result<(String, String)> {
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

fn normalize_remote_spec(remote: &str) -> Option<String> {
    let trimmed = remote.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
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
    let mut process = Command::new(&shell);
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

fn is_interrupted(err: &anyhow::Error) -> bool {
    err.downcast_ref::<io::Error>()
        .is_some_and(|inner| inner.kind() == io::ErrorKind::Interrupted)
}

pub(crate) fn with_session<T>(session: &Arc<Mutex<Session>>, f: impl FnOnce(&Session) -> T) -> Result<T> {
    let guard = session
        .lock()
        .map_err(|_| anyhow!("session lock poisoned"))?;
    Ok(f(&guard))
}

pub(crate) fn with_session_mut<T>(
    session: &Arc<Mutex<Session>>,
    f: impl FnOnce(&mut Session) -> T,
) -> Result<T> {
    let mut guard = session
        .lock()
        .map_err(|_| anyhow!("session lock poisoned"))?;
    Ok(f(&mut guard))
}

pub(crate) fn expand_tilde(path: &str) -> String {
    if path.starts_with('~')
        && let Ok(home) = env::var("HOME")
    {
        if path == "~" {
            return home;
        }
        if let Some(rest) = path.strip_prefix("~/") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}
