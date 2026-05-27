use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct ConfigFile {
    settings: Option<Settings>,
    #[serde(flatten)]
    profiles: BTreeMap<String, ProfileConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct Settings {
    default_profile: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub profiles: BTreeMap<String, ProfileConfig>,
    pub default_profile: Option<String>,
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
            Some(path) => path.to_path_buf(),
            None => default_config_path()
                .ok_or_else(|| anyhow!("could not determine config path"))?,
        };
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config: ConfigFile = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;

        let default_profile = config
            .settings
            .and_then(|s| s.default_profile)
            .filter(|name| !name.is_empty());

        if let Some(ref name) = default_profile
            && !config.profiles.contains_key(name)
        {
            bail!(
                "default_profile `{name}` not found in config profiles"
            );
        }

        Ok(Self {
            profiles: config.profiles,
            default_profile,
        })
    }

    pub fn profile(&self, name: &str) -> Result<&ProfileConfig> {
        self.profiles
            .get(name)
            .ok_or_else(|| anyhow!("profile `{name}` not found in config"))
    }

    pub fn default_profile(&self) -> Option<&str> {
        self.default_profile.as_deref()
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
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config").join("bucketctl").join("config.toml"))
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
