use std::{
    borrow::Cow,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rustyline::{
    Context, Helper,
    completion::{Completer, Pair},
    error::ReadlineError,
    highlight::Highlighter,
    hint::Hinter,
    validate::Validator,
};
use tokio::runtime::Runtime;

use crate::{
    commands,
    session::{Session, resolve_remote_path},
};

pub(crate) struct ReplHelper {
    pub(crate) runtime: Arc<Runtime>,
    pub(crate) session: Arc<Mutex<Session>>,
    pub(crate) colored_prompt: String,
}

impl ReplHelper {
    pub(crate) fn new(runtime: Arc<Runtime>, session: Arc<Mutex<Session>>) -> Self {
        Self {
            runtime,
            session,
            colored_prompt: String::new(),
        }
    }
}

impl Helper for ReplHelper {}

impl Hinter for ReplHelper {
    type Hint = String;
}

impl Highlighter for ReplHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> Cow<'b, str> {
        if default {
            Cow::Borrowed(&self.colored_prompt)
        } else {
            Cow::Borrowed(prompt)
        }
    }
}

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
            let command_names: &[&str] =
                &["help", "pwd", "ls", "cd", "put", "get", "rm", "mkdir", "exit"];
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
    fn complete_remote(
        &self,
        current: &str,
        dirs_only: bool,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let snapshot = commands::with_session(&self.session, |sess| {
            (sess.cwd.clone(), sess.current_bucket().map(ToOwned::to_owned), sess.s3.clone())
        })
        .map_err(to_readline_error)?;
        let (cwd, bucket, s3) = snapshot;
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

pub(crate) fn complete_local_path(current: &str) -> rustyline::Result<(usize, Vec<String>)> {
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
