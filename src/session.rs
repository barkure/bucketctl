use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow, bail};

use crate::{config::ProfileConfig, s3::S3Backend};

#[derive(Clone)]
pub struct Session {
    pub profiles: BTreeMap<String, ProfileConfig>,
    pub profile_name: Option<String>,
    pub s3: Option<S3Backend>,
    pub bucket: Option<String>,
    pub cwd: String,
}

impl Session {
    pub fn new(profiles: BTreeMap<String, ProfileConfig>) -> Self {
        Self {
            profiles,
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
                format!("{profile}:{path} > ")
            }
            (Some(profile), None) => format!("{profile} > "),
            (None, _) => "bucketctl > ".to_owned(),
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
        self.resolve_remote(&format!("{}/{}", remote_dir.trim_end_matches('/'), file_name))
    }

    pub fn resolve_download_target(remote: &str, local: Option<&str>) -> Result<PathBuf> {
        if let Some(local) = local {
            return Ok(PathBuf::from(local));
        }
        let file_name = Path::new(remote)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("remote path must point to an object"))?;
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
    let candidate = if input.starts_with('/') {
        input.to_owned()
    } else if base.is_empty() {
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
