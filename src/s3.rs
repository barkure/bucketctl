use std::{
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Builder, Region, StalledStreamProtectionConfig},
    primitives::ByteStream,
    types::{CommonPrefix, Delete, Object, ObjectIdentifier},
};
use aws_smithy_types::body::SdkBody;
use chrono::{DateTime, Local, Utc};
use futures_util::TryStreamExt;
use http_body::Frame;
use http_body_util::StreamBody;
use indicatif::ProgressBar;
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
    signal,
};
use tokio_util::io::ReaderStream;

use crate::{config::ProfileConfig, ui};

#[derive(Clone)]
pub struct S3Backend {
    client: Client,
}

#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<i64>,
    pub modified: Option<String>,
}

impl S3Backend {
    pub async fn connect(profile: &ProfileConfig) -> Result<Self> {
        let credentials = Credentials::new(
            profile.resolve_access_key()?,
            profile.resolve_secret_key()?,
            None,
            None,
            "bucketctl",
        );
        let mut builder = Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .credentials_provider(credentials)
            .endpoint_url(profile.endpoint.clone())
            .force_path_style(profile.path_style)
            .stalled_stream_protection(
                StalledStreamProtectionConfig::enabled()
                    .upload_enabled(false)
                    .download_enabled(false)
                    .build(),
            );
        builder.set_region(Some(Region::new(profile.region.clone())));

        Ok(Self {
            client: Client::from_conf(builder.build()),
        })
    }

    pub async fn list_prefix(&self, bucket: &str, prefix: &str) -> Result<Vec<RemoteEntry>> {
        let normalized = normalize_prefix(prefix);
        let output = self
            .client
            .list_objects_v2()
            .bucket(bucket)
            .delimiter("/")
            .prefix(normalized.clone())
            .send()
            .await?;

        let dir_markers = output
            .contents()
            .iter()
            .filter_map(|object| dir_marker_metadata(object, &normalized))
            .collect::<std::collections::HashMap<_, _>>();

        let mut entries = Vec::new();
        for common_prefix in output.common_prefixes() {
            if let Some(name) = prefix_name(common_prefix, &normalized) {
                entries.push(RemoteEntry {
                    modified: dir_markers.get(&name).cloned(),
                    name,
                    is_dir: true,
                    size: None,
                });
            }
        }
        for object in output.contents() {
            if let Some(name) = object_name(object, &normalized) {
                entries.push(RemoteEntry {
                    name,
                    is_dir: false,
                    size: object.size(),
                    modified: object.last_modified().and_then(format_local_time),
                });
            }
        }

        entries.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(entries)
    }

    pub async fn put_file(&self, bucket: &str, key: &str, local_path: &Path) -> Result<()> {
        let total_bytes = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("failed to inspect {}", local_path.display()))?
            .len();
        let file = File::open(local_path)
            .await
            .with_context(|| format!("failed to read {}", local_path.display()))?;
        eprintln!("{} {} -> {key}", ui::status_uploading(), local_path.display());
        let progress = ui::transfer_progress_bar(total_bytes)?;
        let progress_for_stream = progress.clone();

        let stream = ReaderStream::with_capacity(file, 256 * 1024).inspect_ok(move |chunk| {
            progress_for_stream.inc(chunk.len() as u64);
        });
        let body = StreamBody::new(stream.map_ok(Frame::data));
        let body = ByteStream::from(SdkBody::from_body_1_x(body));
        let request = self
            .client
            .put_object()
            .bucket(bucket)
            .key(normalize_key(key))
            .content_length(i64::try_from(total_bytes).context("file too large to upload")?)
            .body(body)
            .send();
        tokio::pin!(request);

        tokio::select! {
            result = &mut request => {
                result?;
                ui::print_upload_done(&progress, key, local_path, total_bytes, total_bytes)?;
            }
            _ = signal::ctrl_c() => {
                ui::print_upload_cancelled(&progress, key, local_path, progress.position(), total_bytes)?;
                return Err(io::Error::new(
                    ErrorKind::Interrupted,
                    format!("upload interrupted for `{key}`"),
                ).into());
            }
        }
        Ok(())
    }

    pub async fn create_dir(&self, bucket: &str, key: &str) -> Result<()> {
        let dir_key = normalize_dir_key(key);
        self.client
            .put_object()
            .bucket(bucket)
            .key(dir_key)
            .body(ByteStream::from(Vec::new()))
            .send()
            .await?;
        Ok(())
    }

    pub async fn remote_dir_exists(&self, bucket: &str, key: &str) -> Result<bool> {
        let prefix = normalize_dir_key(key);
        if prefix.is_empty() {
            return Ok(false);
        }

        let output = self
            .client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .max_keys(1)
            .send()
            .await?;

        Ok(
            !output.contents().is_empty()
                || !output.common_prefixes().is_empty()
                || output.key_count().unwrap_or_default() > 0,
        )
    }

    pub async fn get_file(&self, bucket: &str, key: &str, local_path: &Path) -> Result<()> {
        let metadata = self
            .client
            .head_object()
            .bucket(bucket)
            .key(normalize_key(key))
            .send()
            .await?;
        let part_path = download_part_path(local_path);
        let total_bytes = metadata.content_length().and_then(|len| u64::try_from(len).ok());
        let resume_from = existing_part_size(&part_path).await?;

        if let Some(total) = total_bytes {
            if resume_from > total {
                return Err(anyhow!(
                    "partial file {} is larger than remote object; remove it and retry",
                    part_path.display()
                ));
            }
            if resume_from == total {
                tokio::fs::rename(&part_path, local_path)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to move {} to {}",
                            part_path.display(),
                            local_path.display()
                        )
                    })?;
                let progress = ProgressBar::hidden();
                ui::print_download_done(&progress, key, local_path, total, Some(total))?;
                return Ok(());
            }
        }

        let mut request = self.client.get_object().bucket(bucket).key(normalize_key(key));
        if resume_from > 0 {
            request = request.range(format!("bytes={resume_from}-"));
        }
        let output = request.send().await?;

        let mut stream = output.body;
        let mut file = open_download_file(&part_path, resume_from).await?;
        let mut downloaded = resume_from;
        eprintln!("{} {key} -> {}", ui::status_downloading(), local_path.display());
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

        tokio::fs::rename(&part_path, local_path)
            .await
            .with_context(|| {
                format!(
                    "failed to move {} to {}",
                    part_path.display(),
                    local_path.display()
                )
            })?;
        ui::print_download_done(&progress, key, local_path, downloaded, total_bytes)?;
        Ok(())
    }

    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(bucket)
            .key(normalize_key(key))
            .send()
            .await?;
        Ok(())
    }

    pub async fn delete_prefix_recursive(&self, bucket: &str, key: &str) -> Result<usize> {
        let mut keys = self.list_keys_recursive(bucket, key).await?;
        let dir_key = normalize_dir_key(key);
        if !dir_key.is_empty() && !keys.iter().any(|existing| existing == &dir_key) {
            keys.push(dir_key);
        }
        keys.sort();
        keys.dedup();

        let mut deleted = 0;
        for chunk in keys.chunks(1000) {
            let objects = chunk
                .iter()
                .map(|key| ObjectIdentifier::builder().key(key).build())
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if objects.is_empty() {
                continue;
            }
            let delete = Delete::builder().set_objects(Some(objects)).build()?;
            self.client
                .delete_objects()
                .bucket(bucket)
                .delete(delete)
                .send()
                .await?;
            deleted += chunk.len();
        }
        Ok(deleted)
    }
}

fn prefix_name(prefix: &CommonPrefix, base: &str) -> Option<String> {
    let full = prefix.prefix()?;
    let trimmed = full
        .strip_prefix(base)
        .unwrap_or(full)
        .trim_end_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn object_name(object: &Object, base: &str) -> Option<String> {
    let key = object.key()?;
    if key == base {
        return None;
    }
    let trimmed = key.strip_prefix(base).unwrap_or(key);
    if trimmed.is_empty() || trimmed.contains('/') {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn dir_marker_metadata(object: &Object, base: &str) -> Option<(String, String)> {
    let key = object.key()?;
    if key == base || !key.ends_with('/') {
        return None;
    }

    let trimmed = key.strip_prefix(base).unwrap_or(key).trim_end_matches('/');
    if trimmed.is_empty() || trimmed.contains('/') {
        return None;
    }

    Some((
        trimmed.to_owned(),
        object.last_modified().and_then(format_local_time)?,
    ))
}

fn normalize_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

fn normalize_key(key: &str) -> String {
    key.trim_start_matches('/').to_owned()
}

fn normalize_dir_key(key: &str) -> String {
    let normalized = normalize_key(key);
    if normalized.is_empty() || normalized.ends_with('/') {
        normalized
    } else {
        format!("{normalized}/")
    }
}

fn format_local_time(dt: &aws_sdk_s3::primitives::DateTime) -> Option<String> {
    let utc = DateTime::<Utc>::from_timestamp(dt.secs(), dt.subsec_nanos())?;
    let local = utc.with_timezone(&Local);
    let now = Local::now();
    let age = now.signed_duration_since(local);
    let six_months = chrono::Duration::days(183);

    if age >= chrono::Duration::zero() && age <= six_months {
        Some(local.format("%b %e %H:%M").to_string())
    } else {
        Some(local.format("%b %e  %Y").to_string())
    }
}

impl S3Backend {
    async fn list_keys_recursive(&self, bucket: &str, key: &str) -> Result<Vec<String>> {
        let prefix = normalize_dir_key(key);
        let mut continuation = None;
        let mut keys = Vec::new();

        loop {
            let mut request = self.client.list_objects_v2().bucket(bucket).prefix(prefix.clone());
            if let Some(token) = continuation.clone() {
                request = request.continuation_token(token);
            }
            let output = request.send().await?;
            keys.extend(
                output
                    .contents()
                    .iter()
                    .filter_map(|object| object.key().map(ToOwned::to_owned)),
            );

            if output.is_truncated().unwrap_or(false) {
                continuation = output.next_continuation_token().map(ToOwned::to_owned);
            } else {
                break;
            }
        }

        Ok(keys)
    }

}

fn download_part_path(local_path: &Path) -> PathBuf {
    let file_name = local_path
        .file_name()
        .map(|name| format!("{}.download", name.to_string_lossy()))
        .unwrap_or_else(|| ".download".to_owned());
    local_path.with_file_name(file_name)
}

async fn existing_part_size(part_path: &Path) -> Result<u64> {
    match tokio::fs::metadata(part_path).await {
        Ok(metadata) => Ok(metadata.len()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(0),
        Err(err) => Err(err)
            .with_context(|| format!("failed to inspect {}", part_path.display())),
    }
}

async fn open_download_file(part_path: &Path, resume_from: u64) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    if resume_from > 0 {
        options.append(true);
    } else {
        options.truncate(true);
    }
    options
        .open(part_path)
        .await
        .with_context(|| format!("failed to open {}", part_path.display()))
}
