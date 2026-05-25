use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow, bail};
use rustyline::{
    CompletionType, Config,
    Context, Editor, Helper,
    completion::{Completer, Pair},
    error::ReadlineError,
    highlight::Highlighter,
    hint::Hinter,
    history::DefaultHistory,
    validate::Validator,
};
use tokio::runtime::Runtime;

use crate::{
    s3::S3Backend,
    session::{Session, resolve_remote_path},
};

pub fn run_repl(runtime: Arc<Runtime>, session: Arc<Mutex<Session>>) -> Result<()> {
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .completion_prompt_limit(20)
        .build();
    let mut editor = Editor::<ReplHelper, DefaultHistory>::with_config(config)?;
    editor.set_helper(Some(ReplHelper::new(runtime.clone(), session.clone())));

    loop {
        let prompt = {
            let guard = session
                .lock()
                .map_err(|_| anyhow!("session lock poisoned"))?;
            guard.prompt()
        };

        match editor.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);
                match run_command_line(&runtime, &session, line) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(err) => eprintln!("Error: {err:#}"),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => {
                let detached = with_session_mut(&session, |sess| {
                    if sess.current_profile().is_some() {
                        sess.clear_profile();
                        true
                    } else {
                        false
                    }
                })?;
                if detached {
                    continue;
                }
                break;
            }
            Err(err) => return Err(err.into()),
        }
    }

    Ok(())
}

pub fn run_command_line(
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
            let profile_attached =
                with_session(session, |sess| sess.current_profile().is_some())?;
            print_help(profile_attached);
            Ok(false)
        }
        "exit" => Ok(true),
        "pwd" => {
            let location = with_session(session, |sess| {
                if sess.current_profile().is_none() {
                    bail!("`pwd` is only available inside a bucket");
                }
                Ok(sess.location_display())
            })??;
            println!("{location}");
            Ok(false)
        }
        "ls" => {
            let (profile, target, s3, profiles) = with_session(session, |sess| {
                (
                    sess.current_profile().map(ToOwned::to_owned),
                    parts.get(1).cloned(),
                    sess.s3.clone(),
                    sess.list_profiles(),
                )
            })?;

            if profile.is_none() {
                for profile in profiles {
                    println!("{profile}");
                }
            } else {
                let s3 = s3.ok_or_else(|| anyhow!("no profile attached"))?;
                let bucket =
                    with_session(session, |sess| sess.selected_bucket().map(ToOwned::to_owned))??;
                let prefix = with_session(session, |sess| -> Result<_> {
                    sess.resolve_remote(target.as_deref().unwrap_or("."))
                })??;
                for entry in runtime.block_on(s3.list_prefix(&bucket, &prefix))? {
                    if entry.is_dir {
                        if let Some(modified) = entry.modified.as_deref() {
                            println!("{:>10}  {modified}  {}/", "-", entry.name);
                        } else {
                            println!("{:>10}  {:19}  {}/", "-", "", entry.name);
                        }
                    } else if let Some(size) = entry.size {
                        let size = human_bytes(size.max(0) as u64);
                        if let Some(modified) = entry.modified.as_deref() {
                            println!("{size:>10}  {modified}  {}", entry.name);
                        } else {
                            println!("{size:>10}  {}", entry.name);
                        }
                    } else {
                        println!("{}", entry.name);
                    }
                }
            }
            Ok(false)
        }
        "attach" => {
            if parts.len() != 2 {
                bail!("usage: attach <profile>");
            }
            let target = parts[1].as_str();
            let profile =
                with_session(session, |sess| sess.profile_config(target).cloned())??;
            let s3 = runtime.block_on(S3Backend::connect(&profile))?;
            with_session_mut(session, |sess| {
                sess.attach_profile(target.to_owned(), profile.bucket.clone(), s3)
            })?;
            Ok(false)
        }
        "cd" => {
            let target = parts.get(1).map(String::as_str).unwrap_or("/");
            let current_profile =
                with_session(session, |sess| sess.current_profile().map(ToOwned::to_owned))?;
            let new_dir = if current_profile.is_some() {
                with_session_mut(session, |sess| {
                    if target == ".." && sess.in_bucket_root() {
                        return Ok(sess.cwd_display());
                    }
                    sess.change_dir(target)
                })??
            } else if target == "/" || target == "." || target.is_empty() {
                "/".to_owned()
            } else {
                bail!("no profile attached; use `attach <profile>` first");
            };
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
            let local = PathBuf::from(&parts[1]);
            let (bucket, key, s3) = with_session(session, |sess| -> Result<_> {
                let bucket = sess.selected_bucket()?.to_owned();
                let key = sess.resolve_upload_target(&local, parts.get(2).map(String::as_str))?;
                Ok((bucket, key, sess.selected_s3()?.clone()))
            })??;
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

fn print_help(profile_attached: bool) {
    if !profile_attached {
        println!("ls");
        println!("attach <profile>");
        println!("!<command>");
        println!("Ctrl-D");
        println!("exit");
        return;
    }

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
            process.arg("-ilc").arg(command);
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

fn is_interrupted(err: &anyhow::Error) -> bool {
    err.downcast_ref::<io::Error>()
        .is_some_and(|inner| inner.kind() == io::ErrorKind::Interrupted)
}

fn with_session<T>(session: &Arc<Mutex<Session>>, f: impl FnOnce(&Session) -> T) -> Result<T> {
    let guard = session
        .lock()
        .map_err(|_| anyhow!("session lock poisoned"))?;
    Ok(f(&guard))
}

fn with_session_mut<T>(
    session: &Arc<Mutex<Session>>,
    f: impl FnOnce(&mut Session) -> T,
) -> Result<T> {
    let mut guard = session
        .lock()
        .map_err(|_| anyhow!("session lock poisoned"))?;
    Ok(f(&mut guard))
}

struct ReplHelper {
    runtime: Arc<Runtime>,
    session: Arc<Mutex<Session>>,
}

impl ReplHelper {
    fn new(runtime: Arc<Runtime>, session: Arc<Mutex<Session>>) -> Self {
        Self { runtime, session }
    }
}

impl Helper for ReplHelper {}

impl Hinter for ReplHelper {
    type Hint = String;
}

impl Highlighter for ReplHelper {}

impl Validator for ReplHelper {}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let line = &line[..pos];
        if let Some(command) = line.strip_prefix('!') {
            let (start, candidates) = complete_local_path(command)?;
            let pairs = candidates
                .into_iter()
                .map(|candidate| Pair {
                    display: candidate.clone(),
                    replacement: candidate,
                })
                .collect();
            return Ok((start + 1, pairs));
        }

        let parts = shlex::split(line)
            .unwrap_or_else(|| line.split_whitespace().map(ToOwned::to_owned).collect());
        let ends_with_space = line.ends_with(' ');
        let current = if ends_with_space {
            ""
        } else {
            parts.last().map(String::as_str).unwrap_or("")
        };
        let argc = if ends_with_space {
            parts.len() + 1
        } else {
            parts.len()
        };

        if argc <= 1 {
            if current.is_empty() {
                return Ok((line.len(), Vec::new()));
            }
            let profile_attached =
                with_session(&self.session, |sess| sess.current_profile().is_some())
                    .map_err(to_readline_error)?;
            let command_names: &[&str] = if !profile_attached {
                &["help", "ls", "attach", "exit"]
            } else {
                &[
                    "help", "pwd", "ls", "attach", "cd", "put", "get", "rm", "mkdir", "exit",
                ]
            };
            let pairs = command_names
                .iter()
                .filter(|name| name.starts_with(current))
                .map(|name| Pair {
                    display: (*name).to_owned(),
                    replacement: (*name).to_owned(),
                })
                .collect();
            return Ok((line.len() - current.len(), pairs));
        }

        let cmd = parts.first().map(String::as_str).unwrap_or("");
        let base_start = line.len() - current.len();
        let (start, pairs) = match cmd {
            "attach" if argc == 2 => self.complete_profiles(current),
            "cd" | "mkdir" => self.complete_remote(current, true),
            "ls" | "get" | "rm" => self.complete_remote(current, false),
            "put" if argc == 2 => complete_local_path(current).map(|(start, values)| {
                (
                    start,
                    values
                        .into_iter()
                        .map(|candidate| Pair {
                            display: candidate.clone(),
                            replacement: candidate,
                        })
                        .collect(),
                )
            }),
            "put" if argc == 3 => self.complete_remote(current, false),
            _ => Ok((line.len() - current.len(), Vec::new())),
        }?;
        Ok((base_start + start, pairs))
    }
}

impl ReplHelper {
    fn complete_profiles(&self, current: &str) -> rustyline::Result<(usize, Vec<Pair>)> {
        let profiles =
            with_session(&self.session, |sess| sess.list_profiles()).map_err(to_readline_error)?;
        let pairs = profiles
            .into_iter()
            .filter(|profile| profile.starts_with(current))
            .map(|profile| Pair {
                display: profile.clone(),
                replacement: profile,
            })
            .collect();
        Ok((0, pairs))
    }

    fn complete_remote(
        &self,
        current: &str,
        dirs_only: bool,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let snapshot = with_session(&self.session, |sess| {
            (
                sess.cwd.clone(),
                sess.current_profile().is_some(),
                sess.current_bucket().map(ToOwned::to_owned),
                sess.s3.clone(),
            )
        })
        .map_err(to_readline_error)?;
        let (cwd, profile_attached, bucket, s3) = snapshot;
        if !profile_attached {
            return Ok((current.len(), Vec::new()));
        }
        let Some(bucket) = bucket else {
            return Ok((current.len(), Vec::new()));
        };
        let Some(s3) = s3 else {
            return Ok((current.len(), Vec::new()));
        };

        let (parent, needle, replacement_base) = split_remote_completion(current);
        let resolved_parent = resolve_remote_path(&cwd, &parent).map_err(to_readline_error)?;

        let entries = self
            .runtime
            .block_on(s3.list_prefix(&bucket, &resolved_parent))
            .map_err(to_readline_error)?;
        let start = current.len() - needle.len();
        let pairs = entries
            .into_iter()
            .filter(|entry| !dirs_only || entry.is_dir)
            .filter(|entry| entry.name.starts_with(&needle))
            .map(|entry| {
                let mut replacement = format!("{replacement_base}{}", entry.name);
                if entry.is_dir {
                    replacement.push('/');
                }
                Pair {
                    display: replacement.clone(),
                    replacement,
                }
            })
            .collect();
        Ok((start, pairs))
    }
}

fn complete_local_path(current: &str) -> rustyline::Result<(usize, Vec<String>)> {
    let (dir, base) = split_local_completion(current);
    let read_dir = match fs::read_dir(&dir) {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok((current.len().saturating_sub(base.len()), Vec::new()));
        }
        Err(err) => return Err(ReadlineError::Io(err)),
    };
    let mut entries = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(ReadlineError::Io)?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if !name.starts_with(&base) {
            continue;
        }
        let mut candidate = name.to_string();
        if entry.path().is_dir() {
            candidate.push('/');
        }
        entries.push(candidate);
    }
    entries.sort();
    Ok((current.len() - base.len(), entries))
}

fn split_local_completion(current: &str) -> (PathBuf, String) {
    if current.ends_with('/') {
        let dir = PathBuf::from(current.trim_end_matches('/'));
        return (dir, String::new());
    }

    let path = Path::new(current);
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => (
            parent.to_path_buf(),
            name.to_string_lossy().into_owned(),
        ),
        _ => (PathBuf::from("."), current.to_owned()),
    }
}

fn split_remote_completion(current: &str) -> (String, String, String) {
    match current.rsplit_once('/') {
        Some((parent, needle)) => {
            let base = if current.starts_with('/') && parent.is_empty() {
                "/".to_owned()
            } else {
                format!("{parent}/")
            };
            (parent.to_owned(), needle.to_owned(), base)
        }
        None => (String::new(), current.to_owned(), String::new()),
    }
}

fn to_readline_error(err: anyhow::Error) -> ReadlineError {
    ReadlineError::Io(io::Error::other(err.to_string()))
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
