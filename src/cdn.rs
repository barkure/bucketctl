use std::{
    io::{self, ErrorKind},
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use indicatif::ProgressBar;
use reqwest::{
    Client, StatusCode, Url,
    header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE},
};
use tokio::{io::AsyncWriteExt, signal};

use crate::{
    config::ProfileConfig,
    s3::{download_part_path, existing_part_size, normalize_key, open_download_file},
    ui,
};

#[derive(Clone)]
pub struct CdnBackend {
    client: Client,
    base: Url,
}

impl CdnBackend {
    pub fn from_profile(profile: &ProfileConfig) -> Result<Option<Self>> {
        let Some(domain) = profile.cdn_domain.as_deref() else {
            return Ok(None);
        };
        let domain = domain.trim().trim_end_matches('/');
        if !domain.starts_with("http://") && !domain.starts_with("https://") {
            bail!("cdn_domain `{domain}` must start with http:// or https://");
        }
        let base = Url::parse(&format!("{domain}/"))
            .with_context(|| format!("invalid cdn_domain `{domain}`"))?;
        Ok(Some(Self {
            client: Client::new(),
            base,
        }))
    }

    pub fn object_url(&self, key: &str) -> Result<Url> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow!("cdn_domain `{}` cannot be a base URL", self.base))?
            .pop_if_empty()
            .extend(normalize_key(key).split('/'));
        Ok(url)
    }

    pub async fn get_file(&self, key: &str, local_path: &Path) -> Result<()> {
        let url = self.object_url(key)?;
        let part_path = download_part_path(local_path);
        let mut resume_from = existing_part_size(&part_path).await?;

        let mut request = self.client.get(url.clone());
        if resume_from > 0 {
            request = request.header(RANGE, format!("bytes={resume_from}-"));
        }
        let response = request
            .send()
            .await
            .map_err(|err| map_http_err(&err, &url))?;

        let mut total_bytes = content_range_total(&response);
        match response.status() {
            StatusCode::OK => {
                if total_bytes.is_none() {
                    total_bytes = content_length(&response);
                }
                if resume_from > 0 {
                    // server ignored the Range header; restart from scratch
                    resume_from = 0;
                }
            }
            StatusCode::PARTIAL_CONTENT => {
                if total_bytes.is_none() {
                    total_bytes = content_length(&response).map(|len| len + resume_from);
                }
            }
            StatusCode::RANGE_NOT_SATISFIABLE => {
                if let Some(total) = total_bytes {
                    if resume_from == total {
                        rename_part(&part_path, local_path).await?;
                        let progress = ProgressBar::hidden();
                        ui::print_download_done(&progress, key, local_path, total, Some(total))?;
                        return Ok(());
                    }
                    if resume_from > total {
                        bail!(
                            "partial file {} is larger than remote object; remove it and retry",
                            part_path.display()
                        );
                    }
                }
                bail!("cdn download failed for `{key}`: HTTP 416 range not satisfiable");
            }
            StatusCode::NOT_FOUND => bail!("remote object not found: {key}"),
            StatusCode::FORBIDDEN => {
                bail!("access denied via cdn_domain for `{key}`—object may not be public")
            }
            status => bail!("cdn download failed for `{key}`: HTTP {status}"),
        }

        let mut stream = response.bytes_stream();
        let mut file = open_download_file(&part_path, resume_from).await?;
        let mut downloaded = resume_from;
        eprintln!(
            "{} {key} -> {}",
            ui::status_downloading(),
            local_path.display()
        );
        let progress = ui::transfer_progress_bar(total_bytes.unwrap_or(resume_from))?;
        progress.set_position(resume_from);
        loop {
            let chunk = tokio::select! {
                _ = signal::ctrl_c() => {
                    ui::print_download_cancelled(&progress, key, local_path, downloaded, total_bytes, &part_path)?;
                    return Err(io::Error::new(
                        ErrorKind::Interrupted,
                        format!(
                            "download interrupted; partial file kept at {}",
                            part_path.display()
                        ),
                    ).into());
                }
                chunk = stream.next() => chunk,
            };

            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk.with_context(|| format!("failed to read object `{key}`"))?;
            file.write_all(&chunk)
                .await
                .with_context(|| format!("failed to write {}", part_path.display()))?;
            downloaded += chunk.len() as u64;
            progress.inc(chunk.len() as u64);
        }
        file.flush()
            .await
            .with_context(|| format!("failed to flush {}", part_path.display()))?;
        drop(file);

        rename_part(&part_path, local_path).await?;
        ui::print_download_done(&progress, key, local_path, downloaded, total_bytes)?;
        Ok(())
    }
}

fn content_length(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn content_range_total(response: &reqwest::Response) -> Option<u64> {
    let value = response.headers().get(CONTENT_RANGE)?.to_str().ok()?;
    value.rsplit('/').next()?.parse().ok()
}

async fn rename_part(part_path: &Path, local_path: &Path) -> Result<()> {
    tokio::fs::rename(part_path, local_path)
        .await
        .with_context(|| {
            format!(
                "failed to move {} to {}",
                part_path.display(),
                local_path.display()
            )
        })
}

fn map_http_err(err: &reqwest::Error, url: &Url) -> anyhow::Error {
    let host = url.host_str().unwrap_or("cdn");
    if err.is_connect() {
        anyhow!("failed to connect to {host}")
    } else if err.is_timeout() {
        anyhow!("request to {host} timed out")
    } else {
        anyhow!("cdn request to {host} failed: {err}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(domain: &str) -> CdnBackend {
        CdnBackend {
            client: Client::new(),
            base: Url::parse(domain).unwrap(),
        }
    }

    #[test]
    fn object_url_joins_key_segments() {
        let cdn = backend("https://cdn.example.com/");
        assert_eq!(
            cdn.object_url("photos/a b.txt").unwrap().as_str(),
            "https://cdn.example.com/photos/a%20b.txt"
        );
        assert_eq!(
            cdn.object_url("/photos/a.txt").unwrap().as_str(),
            "https://cdn.example.com/photos/a.txt"
        );
    }

    #[test]
    fn object_url_keeps_domain_path_prefix() {
        let cdn = backend("https://cdn.example.com/prefix/");
        assert_eq!(
            cdn.object_url("a.txt").unwrap().as_str(),
            "https://cdn.example.com/prefix/a.txt"
        );
    }

    #[test]
    fn object_url_encodes_special_chars() {
        let cdn = backend("https://cdn.example.com/");
        assert_eq!(
            cdn.object_url("a?b#c.txt").unwrap().as_str(),
            "https://cdn.example.com/a%3Fb%23c.txt"
        );
    }

    #[test]
    fn from_profile_validates_scheme() {
        let profile = ProfileConfig {
            bucket: "b".to_owned(),
            endpoint: "https://s3.example.com".to_owned(),
            region: "auto".to_owned(),
            access_key: "x".to_owned(),
            secret_key: "y".to_owned(),
            path_style: false,
            cdn_domain: Some("cdn.example.com".to_owned()),
        };
        assert!(CdnBackend::from_profile(&profile).is_err());

        let profile = ProfileConfig {
            cdn_domain: Some(" https://cdn.example.com/ ".to_owned()),
            ..profile
        };
        let cdn = CdnBackend::from_profile(&profile).unwrap().unwrap();
        assert_eq!(
            cdn.object_url("a.txt").unwrap().as_str(),
            "https://cdn.example.com/a.txt"
        );
    }
}
