use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub profiles: BTreeMap<String, ProfileConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileConfig {
    pub bucket: String,
    pub endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    #[serde(default)]
    pub path_style: bool,
}

impl AppConfig {
    pub fn load(override_path: Option<&Path>) -> Result<Self> {
        let path = match override_path {
            Some(p) => p.to_path_buf(),
            None => default_config_path()
                .ok_or_else(|| anyhow!("could not determine config path"))?,
        };
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let profiles: BTreeMap<String, ProfileConfig> = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        Ok(Self { profiles })
    }

    pub fn profile(&self, name: &str) -> Result<&ProfileConfig> {
        self.profiles
            .get(name)
            .ok_or_else(|| anyhow!("profile `{name}` not found in config"))
    }
}

impl ProfileConfig {
    pub fn resolve_access_key(&self) -> Result<String> {
        resolve_secret(&self.access_key, "access_key")
    }

    pub fn resolve_secret_key(&self) -> Result<String> {
        resolve_secret(&self.secret_key, "secret_key")
    }
}

fn default_config_path() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("bucketctl")
            .join("config.toml")
    })
}

fn resolve_secret(value: &str, field: &str) -> Result<String> {
    if let Some(name) = value.strip_prefix("env:") {
        let resolved = env::var(name)
            .with_context(|| format!("environment variable `{name}` for `{field}` is not set"))?;
        if resolved.is_empty() {
            bail!("environment variable `{name}` for `{field}` is empty");
        }
        return Ok(resolved);
    }

    if value.is_empty() {
        bail!("`{field}` cannot be empty");
    }

    Ok(value.to_owned())
}
