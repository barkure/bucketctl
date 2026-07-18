use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, anyhow, bail};
use tokio::runtime::Runtime;
use tokio::signal;

use crate::{cdn::CdnBackend, s3::S3Backend, session::Session};

use super::types::ExecMode;

#[derive(Debug, Default)]
pub(crate) struct TransferReport {
    pub ok: usize,
    pub failed: Vec<(String, String)>,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GetRecursiveLayout {
    pub local_root: PathBuf,
    pub rename_style: bool,
}

pub(crate) fn resolve_put_recursive_prefix(
    session: &Session,
    local_root: &Path,
    remote_path: Option<&str>,
    remote_is_existing_dir: bool,
) -> Result<String> {
    if !local_root.is_dir() {
        bail!(
            "recursive upload requires a local directory: {}",
            local_root.display()
        );
    }

    let basename = local_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("local directory must have a name"))?;

    let Some(remote) = remote_path else {
        return session.resolve_remote(basename);
    };

    if remote == "/" || remote == "." {
        return session.resolve_remote(basename);
    }

    if remote.ends_with('/') || remote_is_existing_dir {
        let base = remote.trim_end_matches('/');
        let joined = if base.is_empty() {
            basename.to_owned()
        } else {
            format!("{base}/{basename}")
        };
        return session.resolve_remote(&joined);
    }

    session.resolve_remote(remote)
}

pub(crate) fn join_key_prefix(prefix: &str, relative: &str) -> String {
    let relative = relative.trim_start_matches('/');
    if relative.is_empty() {
        return prefix.trim_end_matches('/').to_owned();
    }
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        relative.to_owned()
    } else {
        format!("{prefix}/{relative}")
    }
}

pub(crate) fn last_path_segment(prefix: &str) -> Option<String> {
    let trimmed = prefix.trim_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        trimmed.rsplit('/').next().map(str::to_owned)
    }
}

pub(crate) fn resolve_get_recursive_layout(
    remote_prefix: &str,
    local_arg: Option<&Path>,
) -> Result<GetRecursiveLayout> {
    let is_bucket_root = remote_prefix.trim_matches('/').is_empty();
    let last_segment = last_path_segment(remote_prefix);

    match local_arg {
        None if is_bucket_root => {
            bail!(
                "recursive download of the bucket root requires an explicit local directory (e.g. `get -r / ./backup`)"
            );
        }
        None => {
            let segment = last_segment.ok_or_else(|| {
                anyhow!("recursive download requires a named remote prefix or an explicit local directory")
            })?;
            Ok(GetRecursiveLayout {
                local_root: PathBuf::from(segment),
                rename_style: false,
            })
        }
        Some(local) if local.as_os_str() == "." => {
            let segment = last_segment.unwrap_or_else(|| ".".to_owned());
            Ok(GetRecursiveLayout {
                local_root: PathBuf::from(segment),
                rename_style: false,
            })
        }
        Some(local) => {
            let ends_with_slash = local.to_str().is_some_and(|value| value.ends_with('/'));
            let is_existing_dir = local.is_dir();
            if is_bucket_root || ends_with_slash || is_existing_dir {
                let segment = last_segment.unwrap_or_default();
                let local_root = if segment.is_empty() {
                    local.to_path_buf()
                } else {
                    local.join(segment)
                };
                Ok(GetRecursiveLayout {
                    local_root,
                    rename_style: false,
                })
            } else {
                Ok(GetRecursiveLayout {
                    local_root: local.to_path_buf(),
                    rename_style: true,
                })
            }
        }
    }
}

pub(crate) fn compute_get_local_path(layout: &GetRecursiveLayout, relative_key: &str) -> PathBuf {
    layout.local_root.join(relative_key)
}

pub(crate) fn relative_key_from_prefix(prefix: &str, key: &str) -> Option<String> {
    if key.ends_with('/') {
        return None;
    }

    let normalized = normalize_list_prefix(prefix);
    if normalized.is_empty() {
        return Some(key.trim_start_matches('/').to_owned());
    }

    key.strip_prefix(&normalized)
        .or_else(|| key.strip_prefix(normalized.trim_end_matches('/')))
        .map(|value| value.trim_start_matches('/').to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_list_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

pub(crate) fn collect_regular_files(root: &Path) -> Result<(Vec<PathBuf>, usize)> {
    let mut files = Vec::new();
    let mut skipped = 0;
    collect_files_recursive(root, &mut files, &mut skipped)?;
    files.sort();
    Ok((files, skipped))
}

fn collect_files_recursive(
    current: &Path,
    files: &mut Vec<PathBuf>,
    skipped: &mut usize,
) -> Result<()> {
    for entry in std::fs::read_dir(current)
        .with_context(|| format!("failed to read {}", current.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            eprintln!("warning: skipping symlink {}", path.display());
            *skipped += 1;
            continue;
        }
        if file_type.is_dir() {
            collect_files_recursive(&path, files, skipped)?;
        } else if file_type.is_file() {
            files.push(path);
        } else {
            eprintln!("warning: skipping non-regular file {}", path.display());
            *skipped += 1;
        }
    }
    Ok(())
}

pub(crate) fn relative_local_path(root: &Path, file: &Path) -> Result<String> {
    let rel = file
        .strip_prefix(root)
        .with_context(|| format!("{} is not under {}", file.display(), root.display()))?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn execute_put_recursive(
    runtime: &Runtime,
    s3: &S3Backend,
    bucket: &str,
    local_root: &Path,
    key_prefix: &str,
    mode: ExecMode,
) -> Result<(TransferReport, bool)> {
    let (files, skipped_symlinks) = collect_regular_files(local_root)?;
    let local_root = local_root.to_path_buf();
    let key_prefix = key_prefix.to_owned();
    let bucket = bucket.to_owned();
    let s3 = s3.clone();

    runtime.block_on(async move {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_flag = cancelled.clone();
        let ctrl_c = tokio::spawn(async move {
            if signal::ctrl_c().await.is_ok() {
                cancel_flag.store(true, Ordering::SeqCst);
            }
        });

        let mut report = TransferReport {
            skipped: skipped_symlinks,
            ..TransferReport::default()
        };

        for file in files {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }
            let relative = match relative_local_path(&local_root, &file) {
                Ok(value) => value,
                Err(err) => {
                    report
                        .failed
                        .push((file.display().to_string(), err.to_string()));
                    eprintln!("Error: {}: {err:#}", file.display());
                    continue;
                }
            };
            let key = join_key_prefix(&key_prefix, &relative);
            match s3.put_file(&bucket, &key, &file).await {
                Ok(()) => report.ok += 1,
                Err(err) if is_interrupted(&err) => {
                    cancelled.store(true, Ordering::SeqCst);
                    break;
                }
                Err(err) => {
                    let message = err.to_string();
                    eprintln!("Error: {key}: {message}");
                    report.failed.push((key, message));
                }
            }
        }

        ctrl_c.abort();
        let interrupted = cancelled.load(Ordering::SeqCst);
        print_transfer_summary(&report, interrupted, mode);
        Ok((report, interrupted))
    })
}

pub(crate) fn execute_get_recursive(
    runtime: &Runtime,
    s3: &S3Backend,
    cdn: Option<&CdnBackend>,
    bucket: &str,
    remote_prefix: &str,
    layout: &GetRecursiveLayout,
    mode: ExecMode,
) -> Result<(TransferReport, bool)> {
    let layout = layout.clone();
    let remote_prefix = remote_prefix.to_owned();
    let bucket = bucket.to_owned();
    let s3 = s3.clone();
    let cdn = cdn.cloned();

    runtime.block_on(async move {
        let keys = s3.list_keys_recursive(&bucket, &remote_prefix).await?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_flag = cancelled.clone();
        let ctrl_c = tokio::spawn(async move {
            if signal::ctrl_c().await.is_ok() {
                cancel_flag.store(true, Ordering::SeqCst);
            }
        });

        let mut report = TransferReport::default();
        let mut keys: Vec<String> = keys
            .into_iter()
            .filter_map(|key| relative_key_from_prefix(&remote_prefix, &key))
            .collect();
        keys.sort();
        keys.dedup();

        for relative in keys {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }
            let key = join_key_prefix(&remote_prefix, &relative);
            let local_path = compute_get_local_path(&layout, &relative);
            if let Some(parent) = local_path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create local directory {}", parent.display())
                })?;
            }
            let result = match &cdn {
                Some(cdn) => cdn.get_file(&key, &local_path).await,
                None => s3.get_file(&bucket, &key, &local_path).await,
            };
            match result {
                Ok(()) => report.ok += 1,
                Err(err) if is_interrupted(&err) => {
                    cancelled.store(true, Ordering::SeqCst);
                    break;
                }
                Err(err) => {
                    let message = err.to_string();
                    eprintln!("Error: {key}: {message}");
                    report.failed.push((key, message));
                }
            }
        }

        ctrl_c.abort();
        let interrupted = cancelled.load(Ordering::SeqCst);
        print_transfer_summary(&report, interrupted, mode);
        Ok((report, interrupted))
    })
}

fn print_transfer_summary(report: &TransferReport, interrupted: bool, mode: ExecMode) {
    let mut parts = vec![format!("{} succeeded", report.ok)];
    if report.skipped > 0 {
        parts.push(format!("{} skipped", report.skipped));
    }
    if !report.failed.is_empty() {
        parts.push(format!("{} failed", report.failed.len()));
    }
    if interrupted {
        parts.push("interrupted".to_owned());
    }
    let _ = mode;
    println!("transfer summary: {}", parts.join(", "));
}

pub(crate) fn finish_recursive(
    mode: ExecMode,
    report: TransferReport,
    interrupted: bool,
) -> Result<super::types::ExecOutcome> {
    if interrupted {
        return match mode {
            ExecMode::Interactive => Ok(super::types::ExecOutcome::Continue),
            ExecMode::NonInteractive => Ok(super::types::ExecOutcome::ExitCode(130)),
        };
    }
    if !report.failed.is_empty() {
        bail!("{} transfer(s) failed", report.failed.len());
    }
    Ok(super::types::ExecOutcome::Continue)
}

fn is_interrupted(err: &anyhow::Error) -> bool {
    err.downcast_ref::<io::Error>()
        .is_some_and(|inner| inner.kind() == io::ErrorKind::Interrupted)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::session::Session;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bucketctl-transfer-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn join_key_prefix_joins_relative_paths() {
        assert_eq!(join_key_prefix("foo", "a/b.txt"), "foo/a/b.txt");
        assert_eq!(join_key_prefix("foo/", "a/b.txt"), "foo/a/b.txt");
        assert_eq!(join_key_prefix("", "a/b.txt"), "a/b.txt");
    }

    #[test]
    fn resolve_put_prefix_omitted_uses_basename() {
        let local_root = temp_dir("foo");
        let basename = local_root.file_name().unwrap().to_string_lossy();
        let session = Session::new(BTreeMap::new(), None);
        let prefix = resolve_put_recursive_prefix(&session, &local_root, None, false).unwrap();
        assert_eq!(prefix, basename);
        let _ = std::fs::remove_dir_all(local_root);
    }

    #[test]
    fn resolve_put_prefix_rename_style_for_new_remote_dir() {
        let local_root = temp_dir("foo-rename");
        let session = Session::new(BTreeMap::new(), None);
        let prefix =
            resolve_put_recursive_prefix(&session, &local_root, Some("bar"), false).unwrap();
        assert_eq!(prefix, "bar");
        let _ = std::fs::remove_dir_all(local_root);
    }

    #[test]
    fn resolve_put_prefix_places_basename_under_existing_dir() {
        let local_root = temp_dir("foo-existing");
        let basename = local_root.file_name().unwrap().to_string_lossy();
        let session = Session::new(BTreeMap::new(), None);
        let prefix =
            resolve_put_recursive_prefix(&session, &local_root, Some("bar"), true).unwrap();
        assert_eq!(prefix, format!("bar/{basename}"));
        let _ = std::fs::remove_dir_all(local_root);
    }

    #[test]
    fn resolve_put_prefix_trailing_slash_places_basename() {
        let local_root = temp_dir("foo-trailing");
        let basename = local_root.file_name().unwrap().to_string_lossy();
        let session = Session::new(BTreeMap::new(), None);
        let prefix =
            resolve_put_recursive_prefix(&session, &local_root, Some("bar/"), false).unwrap();
        assert_eq!(prefix, format!("bar/{basename}"));
        let _ = std::fs::remove_dir_all(local_root);
    }

    #[test]
    fn get_layout_defaults_to_last_segment() {
        let layout = resolve_get_recursive_layout("photos/sub", None).unwrap();
        assert_eq!(layout.local_root, PathBuf::from("sub"));
        assert!(!layout.rename_style);
    }

    #[test]
    fn get_layout_rejects_bucket_root_without_local() {
        assert!(resolve_get_recursive_layout("", None).is_err());
        assert!(resolve_get_recursive_layout("/", None).is_err());
    }

    #[test]
    fn get_layout_existing_dir_includes_last_segment() {
        let dir = temp_dir("get-layout-existing");
        let layout = resolve_get_recursive_layout("photos", Some(&dir)).unwrap();
        assert_eq!(layout.local_root, dir.join("photos"));
        assert!(!layout.rename_style);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn get_layout_rename_style_for_new_local_path() {
        let layout = resolve_get_recursive_layout("photos", Some(Path::new("./out"))).unwrap();
        assert_eq!(layout.local_root, PathBuf::from("./out"));
        assert!(layout.rename_style);
    }

    #[test]
    fn relative_key_strips_prefix_and_skips_markers() {
        assert_eq!(
            relative_key_from_prefix("photos", "photos/a.txt").as_deref(),
            Some("a.txt")
        );
        assert!(relative_key_from_prefix("photos", "photos/").is_none());
    }

    #[test]
    fn compute_get_local_path_joins_under_root() {
        let layout = GetRecursiveLayout {
            local_root: PathBuf::from("photos"),
            rename_style: false,
        };
        assert_eq!(
            compute_get_local_path(&layout, "a/b.txt"),
            PathBuf::from("photos/a/b.txt")
        );
    }
}
