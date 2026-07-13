use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow, bail};
use tokio::runtime::Runtime;

use crate::{config::ProfileConfig, s3::S3Backend};

#[derive(Clone)]
pub struct Session {
    pub profiles: BTreeMap<String, ProfileConfig>,
    pub default_profile: Option<String>,
    pub profile_name: Option<String>,
    pub s3: Option<S3Backend>,
    pub bucket: Option<String>,
    pub cwd: String,
}

impl Session {
    pub fn new(profiles: BTreeMap<String, ProfileConfig>, default_profile: Option<String>) -> Self {
        Self {
            profiles,
            default_profile,
            profile_name: None,
            s3: None,
            bucket: None,
            cwd: String::new(),
        }
    }

    pub fn prompt(&self) -> String {
        match (&self.profile_name, &self.bucket) {
            (Some(profile), Some(_bucket)) => {
                let path = if self.cwd.is_empty() {
                    "/".to_owned()
                } else {
                    format!("/{}", self.cwd)
                };
                format!("{profile}:{path}> ")
            }
            (Some(profile), None) => format!("{profile}> "),
            (None, _) => "bucketctl> ".to_owned(),
        }
    }

    pub fn selected_s3(&self) -> Result<&S3Backend> {
        self.s3
            .as_ref()
            .ok_or_else(|| anyhow!("no profile attached"))
    }

    pub fn selected_bucket(&self) -> Result<&str> {
        self.bucket
            .as_deref()
            .ok_or_else(|| anyhow!("no profile attached"))
    }

    pub fn attach_profile(&mut self, profile_name: String, bucket: String, s3: S3Backend) {
        self.profile_name = Some(profile_name);
        self.bucket = Some(bucket);
        self.s3 = Some(s3);
        self.cwd.clear();
    }

    pub fn current_bucket(&self) -> Option<&str> {
        self.bucket.as_deref()
    }

    pub fn list_profiles(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    pub fn profile_config(&self, name: &str) -> Result<&ProfileConfig> {
        self.profiles
            .get(name)
            .ok_or_else(|| anyhow!("profile `{name}` not found in config"))
    }

    pub fn has_profile(&self, name: &str) -> bool {
        self.profiles.contains_key(name)
    }

    pub fn default_profile_name(&self) -> Option<&str> {
        self.default_profile.as_deref()
    }

    pub fn resolve_remote(&self, input: &str) -> Result<String> {
        resolve_remote_path(&self.cwd, input)
    }

    pub fn resolve_upload_target(&self, local: &Path, remote: Option<&str>) -> Result<String> {
        let file_name = local
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("local path must point to a file name"))?;

        if let Some(remote) = remote {
            if remote.ends_with('/') {
                return self.resolve_remote(&format!("{remote}{file_name}"));
            }
            return self.resolve_remote(remote);
        }
        self.resolve_remote(file_name)
    }

    pub fn resolve_upload_target_in_dir(&self, local: &Path, remote_dir: &str) -> Result<String> {
        let file_name = local
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("local path must point to a file name"))?;
        self.resolve_remote(&format!(
            "{}/{}",
            remote_dir.trim_end_matches('/'),
            file_name
        ))
    }

    pub fn resolve_download_target(remote: &str, local: Option<&str>) -> Result<PathBuf> {
        let file_name = Path::new(remote)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("remote path must point to an object"))?;

        if let Some(local) = local {
            let local_path = PathBuf::from(expand_tilde(local));
            if local_path.is_dir() {
                return Ok(local_path.join(file_name));
            }
            return Ok(local_path);
        }
        Ok(PathBuf::from(file_name))
    }

    pub fn change_dir(&mut self, target: &str) -> Result<String> {
        let resolved = resolve_remote_path(&self.cwd, target)?;
        self.cwd = if resolved.is_empty() {
            String::new()
        } else {
            format!("{}/", resolved.trim_end_matches('/'))
        };
        Ok(self.cwd_display())
    }

    pub fn cwd_display(&self) -> String {
        if self.cwd.is_empty() {
            "/".to_owned()
        } else {
            format!("/{}", self.cwd.trim_end_matches('/'))
        }
    }

    pub fn location_display(&self) -> String {
        match (&self.profile_name, &self.bucket) {
            (None, _) => "/".to_owned(),
            (Some(profile), Some(_bucket)) => format!("{profile}:{}", self.cwd_display()),
            (Some(profile), None) => format!("{profile}:/"),
        }
    }
}

pub fn resolve_remote_path(cwd: &str, input: &str) -> Result<String> {
    let base = if input.starts_with('/') {
        String::new()
    } else {
        cwd.trim_end_matches('/').to_owned()
    };
    let candidate = if input.starts_with('/') || base.is_empty() {
        input.to_owned()
    } else if input.is_empty() {
        format!("/{base}")
    } else {
        format!("/{base}/{input}")
    };

    let mut parts = Vec::new();
    for segment in candidate.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    bail!("path escapes bucket root");
                }
            }
            other => parts.push(other),
        }
    }

    Ok(parts.join("/"))
}

pub fn expand_tilde(path: &str) -> String {
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

pub fn with_session<T>(session: &Arc<Mutex<Session>>, f: impl FnOnce(&Session) -> T) -> Result<T> {
    let guard = session
        .lock()
        .map_err(|_| anyhow!("session lock poisoned"))?;
    Ok(f(&guard))
}

pub fn with_session_mut<T>(
    session: &Arc<Mutex<Session>>,
    f: impl FnOnce(&mut Session) -> T,
) -> Result<T> {
    let mut guard = session
        .lock()
        .map_err(|_| anyhow!("session lock poisoned"))?;
    Ok(f(&mut guard))
}

pub fn attach_profile_for_command(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_remote_path_absolute_and_relative() {
        assert_eq!(resolve_remote_path("", "foo").unwrap(), "foo");
        assert_eq!(resolve_remote_path("a/b/", "c").unwrap(), "a/b/c");
        assert_eq!(resolve_remote_path("a", "/x").unwrap(), "x");
        assert_eq!(resolve_remote_path("a/b", "..").unwrap(), "a");
        assert_eq!(resolve_remote_path("", ".").unwrap(), "");
    }

    #[test]
    fn resolve_remote_path_rejects_escape() {
        assert!(resolve_remote_path("", "..").is_err());
    }

    #[test]
    fn expand_tilde_home_and_prefix() {
        let home = env::var("HOME").expect("HOME");
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/Downloads"), format!("{home}/Downloads"));
        assert_eq!(expand_tilde("/tmp/x"), "/tmp/x");
    }

    #[test]
    fn resolve_download_target_expands_tilde() {
        let home = env::var("HOME").expect("HOME");
        let target = Session::resolve_download_target("path/file.txt", Some("~/out")).unwrap();
        assert_eq!(target, PathBuf::from(format!("{home}/out")));
    }

    #[test]
    fn resolve_download_target_dir_joins_filename() {
        let dir = env::temp_dir();
        let target = Session::resolve_download_target(
            "remote/obj.txt",
            Some(dir.as_os_str().to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(target, dir.join("obj.txt"));
    }

    #[test]
    fn resolve_upload_target_defaults_to_basename() {
        let sess = Session::new(BTreeMap::new(), None);
        let local = Path::new("/tmp/hello.txt");
        assert_eq!(
            sess.resolve_upload_target(local, None).unwrap(),
            "hello.txt"
        );
    }
}
