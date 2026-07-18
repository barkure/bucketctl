use std::{collections::BTreeMap, env, fs, path::Path};

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
    #[serde(default)]
    pub cdn_domain: Option<String>,
}

impl AppConfig {
    pub fn load(override_path: Option<&Path>) -> Result<Self> {
        let path = match override_path {
            Some(path) => path.to_path_buf(),
            None => crate::paths::default_config_path()
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
            bail!("default_profile `{name}` not found in config profiles");
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

pub fn init_config(override_path: Option<&Path>, force: bool) -> Result<()> {
    let path = match override_path {
        Some(path) => path.to_path_buf(),
        None => crate::paths::default_config_path()
            .ok_or_else(|| anyhow!("could not determine config path"))?,
    };

    if path.exists() && !force {
        bail!(
            "config file already exists at {}; use --force to overwrite",
            path.display()
        );
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    fs::write(&path, CONFIG_TEMPLATE)
        .with_context(|| format!("failed to write config template to {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

const CONFIG_TEMPLATE: &str = r#"[settings]
# default_profile = "myprofile"

[myprofile]
bucket = "my-bucket"
endpoint = "https://s3.example.com"
region = "auto"
access_key = "env:ACCESS_KEY"
secret_key = "env:SECRET_KEY"
path_style = false
# download objects through your CDN domain instead of the S3 endpoint
# cdn_domain = "https://cdn.example.com"
"#;

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
