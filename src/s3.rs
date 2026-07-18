use std::{
    collections::{BTreeSet, HashMap},
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, anyhow};
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Builder, Region, StalledStreamProtectionConfig},
    primitives::ByteStream,
    types::{
        CommonPrefix, CompletedMultipartUpload, CompletedPart, Delete, Object, ObjectIdentifier,
    },
};
use aws_smithy_runtime_api::{client::result::SdkError, http::Response as HttpResponse};
use aws_smithy_types::{body::SdkBody, byte_stream::Length};
use chrono::{DateTime, Local, Utc};
use futures_util::TryStreamExt;
use http_body::Frame;
use http_body_util::StreamBody;
use indicatif::ProgressBar;
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
    signal,
    sync::Semaphore,
    task::JoinSet,
};
use tokio_util::io::ReaderStream;

use crate::{config::ProfileConfig, ui};

const PART_SIZE: u64 = 8 * 1024 * 1024;
const MIN_NON_LAST_PART_SIZE: u64 = 5 * 1024 * 1024;
const MAX_PARTS: u64 = 10_000;
const MAX_PART_CONCURRENCY: usize = 4;
const MIB: u64 = 1024 * 1024;

#[derive(Clone)]
pub struct S3Backend {
    client: Client,
    endpoint: String,
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
            endpoint: profile.endpoint.clone(),
        })
    }

    fn map_err<E, R>(&self, err: SdkError<E, R>, bucket: &str, key: Option<&str>) -> anyhow::Error
    where
        E: std::error::Error + Send + Sync + 'static,
        R: ResponseStatus,
    {
        map_s3_err(err, bucket, key, &self.endpoint)
    }

    pub async fn list_prefix(&self, bucket: &str, prefix: &str) -> Result<Vec<RemoteEntry>> {
        let normalized = normalize_prefix(prefix);
        let mut continuation = None;
        let mut dir_markers: HashMap<String, String> = HashMap::new();
        let mut dir_names: BTreeSet<String> = BTreeSet::new();
        let mut files: Vec<RemoteEntry> = Vec::new();

        loop {
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(bucket)
                .delimiter("/")
                .prefix(normalized.clone());
            if let Some(token) = continuation {
                request = request.continuation_token(token);
            }
            let output = request
                .send()
                .await
                .map_err(|err| self.map_err(err, bucket, Some(prefix)))?;

            for object in output.contents() {
                if let Some((name, modified)) = dir_marker_metadata(object, &normalized) {
                    dir_markers.insert(name, modified);
                }
            }
            for common_prefix in output.common_prefixes() {
                if let Some(name) = prefix_name(common_prefix, &normalized) {
                    dir_names.insert(name);
                }
            }
            for object in output.contents() {
                if let Some(name) = object_name(object, &normalized) {
                    files.push(RemoteEntry {
                        name,
                        is_dir: false,
                        size: object.size(),
                        modified: object.last_modified().and_then(format_local_time),
                    });
                }
            }

            if output.is_truncated().unwrap_or(false) {
                continuation = output.next_continuation_token().map(str::to_owned);
            } else {
                break;
            }
        }

        Ok(merge_remote_entries(dir_markers, dir_names, files))
    }

    pub async fn list_for_completion(
        &self,
        bucket: &str,
        parent_dir_prefix: &str,
        needle: &str,
        max_pages: usize,
    ) -> Result<Vec<RemoteEntry>> {
        let strip_base = normalize_prefix(parent_dir_prefix);
        let list_prefix_str = format!("{strip_base}{needle}");
        let mut continuation = None;
        let mut dir_markers: HashMap<String, String> = HashMap::new();
        let mut dir_names: BTreeSet<String> = BTreeSet::new();
        let mut files: Vec<RemoteEntry> = Vec::new();
        let mut pages = 0;

        loop {
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(bucket)
                .delimiter("/")
                .prefix(list_prefix_str.clone());
            if let Some(token) = continuation {
                request = request.continuation_token(token);
            }
            let output = request
                .send()
                .await
                .map_err(|err| self.map_err(err, bucket, Some(parent_dir_prefix)))?;
            pages += 1;

            for object in output.contents() {
                if let Some((name, modified)) = dir_marker_metadata(object, &strip_base) {
                    dir_markers.insert(name, modified);
                }
            }
            for common_prefix in output.common_prefixes() {
                if let Some(name) = prefix_name(common_prefix, &strip_base) {
                    dir_names.insert(name);
                }
            }
            for object in output.contents() {
                if let Some(name) = object_name(object, &strip_base) {
                    files.push(RemoteEntry {
                        name,
                        is_dir: false,
                        size: object.size(),
                        modified: object.last_modified().and_then(format_local_time),
                    });
                }
            }

            let truncated = output.is_truncated().unwrap_or(false);
            if !truncated || pages >= max_pages {
                break;
            }
            continuation = output.next_continuation_token().map(str::to_owned);
        }

        Ok(merge_remote_entries(dir_markers, dir_names, files))
    }

    pub async fn put_file(&self, bucket: &str, key: &str, local_path: &Path) -> Result<()> {
        let total_bytes = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("failed to inspect {}", local_path.display()))?
            .len();
        if total_bytes <= PART_SIZE {
            self.put_object_simple(bucket, key, local_path, total_bytes)
                .await
        } else {
            self.put_object_multipart(bucket, key, local_path, total_bytes)
                .await
        }
    }

    async fn put_object_simple(
        &self,
        bucket: &str,
        key: &str,
        local_path: &Path,
        total_bytes: u64,
    ) -> Result<()> {
        let file = File::open(local_path)
            .await
            .with_context(|| format!("failed to read {}", local_path.display()))?;
        eprintln!(
            "{} {} -> {key}",
            ui::status_uploading(),
            local_path.display()
        );
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
                result.map_err(|err| self.map_err(err, bucket, Some(key)))?;
                ui::print_upload_done(&progress, key, local_path, total_bytes, total_bytes)?;
            }
            _ = signal::ctrl_c() => {
                ui::print_upload_cancelled(&progress, key, local_path, progress.position(), total_bytes)?;
                return Err(upload_interrupted(key));
            }
        }
        Ok(())
    }

    async fn put_object_multipart(
        &self,
        bucket: &str,
        key: &str,
        local_path: &Path,
        total_bytes: u64,
    ) -> Result<()> {
        let normalized_key = normalize_key(key);
        let planned = part_plan(total_bytes, PART_SIZE);
        eprintln!(
            "{} {} -> {key}",
            ui::status_uploading(),
            local_path.display()
        );
        let progress = ui::transfer_progress_bar(total_bytes)?;

        let create_output = self
            .client
            .create_multipart_upload()
            .bucket(bucket)
            .key(&normalized_key)
            .send()
            .await
            .map_err(|err| self.map_err(err, bucket, Some(key)))?;
        let upload_id = create_output
            .upload_id()
            .ok_or_else(|| anyhow!("multipart upload missing upload_id"))?
            .to_owned();

        let mut guard = MultipartUploadGuard::new(
            self.client.clone(),
            bucket.to_owned(),
            normalized_key.clone(),
            upload_id.clone(),
            tokio::runtime::Handle::current(),
        );

        let cancelled = Arc::new(AtomicBool::new(false));
        let mut completed_parts = Vec::with_capacity(planned.len());
        let mut next_part = 0usize;
        let mut join_set: JoinSet<Result<CompletedPart>> = JoinSet::new();
        let semaphore = Arc::new(Semaphore::new(MAX_PART_CONCURRENCY));

        loop {
            while next_part < planned.len()
                && join_set.len() < MAX_PART_CONCURRENCY
                && !cancelled.load(Ordering::SeqCst)
            {
                let (part_number, offset, len) = planned[next_part];
                next_part += 1;
                let permit = semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .context("failed to acquire upload permit")?;
                let client = self.client.clone();
                let bucket = bucket.to_owned();
                let normalized_key = normalized_key.clone();
                let upload_id = upload_id.clone();
                let local_path = local_path.to_path_buf();
                let progress = progress.clone();
                let cancelled = cancelled.clone();

                join_set.spawn(async move {
                    let _permit = permit;
                    if cancelled.load(Ordering::SeqCst) {
                        return Err(upload_interrupted(&normalized_key));
                    }
                    let body = ByteStream::read_from()
                        .path(&local_path)
                        .offset(offset)
                        .length(Length::Exact(len))
                        .build()
                        .await
                        .with_context(|| {
                            format!(
                                "failed to read {} for part {part_number}",
                                local_path.display()
                            )
                        })?;
                    let output = client
                        .upload_part()
                        .bucket(&bucket)
                        .key(&normalized_key)
                        .upload_id(&upload_id)
                        .part_number(part_number as i32)
                        .content_length(i64::try_from(len).context("part too large")?)
                        .body(body)
                        .send()
                        .await
                        .with_context(|| format!("failed to upload part {part_number}"))?;
                    let etag = output
                        .e_tag()
                        .ok_or_else(|| anyhow!("multipart part {part_number} missing ETag"))?;
                    progress.inc(len);
                    Ok(CompletedPart::builder()
                        .part_number(part_number as i32)
                        .e_tag(etag)
                        .build())
                });
            }

            if join_set.is_empty() {
                if cancelled.load(Ordering::SeqCst) || next_part >= planned.len() {
                    break;
                }
                continue;
            }

            tokio::select! {
                _ = signal::ctrl_c(), if !cancelled.load(Ordering::SeqCst) => {
                    cancelled.store(true, Ordering::SeqCst);
                }
                res = join_set.join_next() => {
                    match res {
                        Some(Ok(Ok(part))) => completed_parts.push(part),
                        Some(Ok(Err(err))) => {
                            cancelled.store(true, Ordering::SeqCst);
                            join_set.abort_all();
                            while join_set.join_next().await.is_some() {}
                            guard.abort().await;
                            return Err(err);
                        }
                        Some(Err(err)) => {
                            cancelled.store(true, Ordering::SeqCst);
                            join_set.abort_all();
                            while join_set.join_next().await.is_some() {}
                            guard.abort().await;
                            return Err(err.into());
                        }
                        None => {}
                    }
                }
            }

            if cancelled.load(Ordering::SeqCst) && join_set.is_empty() {
                break;
            }
        }

        if cancelled.load(Ordering::SeqCst) {
            join_set.abort_all();
            while join_set.join_next().await.is_some() {}
            guard.abort().await;
            ui::print_upload_cancelled(
                &progress,
                key,
                local_path,
                progress.position(),
                total_bytes,
            )?;
            return Err(upload_interrupted(key));
        }

        if completed_parts.len() != planned.len() {
            guard.abort().await;
            return Err(anyhow!(
                "multipart upload incomplete: expected {} parts, uploaded {}",
                planned.len(),
                completed_parts.len()
            ));
        }

        sort_completed_parts(&mut completed_parts);
        let multipart_upload = CompletedMultipartUpload::builder()
            .set_parts(Some(completed_parts))
            .build();
        self.client
            .complete_multipart_upload()
            .bucket(bucket)
            .key(&normalized_key)
            .upload_id(&upload_id)
            .multipart_upload(multipart_upload)
            .send()
            .await
            .context("failed to complete multipart upload")?;

        guard.mark_completed();
        ui::print_upload_done(&progress, key, local_path, total_bytes, total_bytes)?;
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
            .await
            .map_err(|err| self.map_err(err, bucket, Some(key)))?;
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
            .await
            .map_err(|err| self.map_err(err, bucket, Some(key)))?;

        Ok(!output.contents().is_empty()
            || !output.common_prefixes().is_empty()
            || output.key_count().unwrap_or_default() > 0)
    }

    pub async fn get_file(&self, bucket: &str, key: &str, local_path: &Path) -> Result<()> {
        let metadata = self
            .client
            .head_object()
            .bucket(bucket)
            .key(normalize_key(key))
            .send()
            .await
            .map_err(|err| self.map_err(err, bucket, Some(key)))?;
        let part_path = download_part_path(local_path);
        let total_bytes = metadata
            .content_length()
            .and_then(|len| u64::try_from(len).ok());
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

        let mut request = self
            .client
            .get_object()
            .bucket(bucket)
            .key(normalize_key(key));
        if resume_from > 0 {
            request = request.range(format!("bytes={resume_from}-"));
        }
        let output = request
            .send()
            .await
            .map_err(|err| self.map_err(err, bucket, Some(key)))?;

        let mut stream = output.body;
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
            .await
            .map_err(|err| self.map_err(err, bucket, Some(key)))?;
        Ok(())
    }

    pub(crate) async fn list_keys_for_delete(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<Vec<String>> {
        let mut keys = self.list_keys_recursive(bucket, key).await?;
        let dir_key = normalize_dir_key(key);
        if !dir_key.is_empty() && !keys.iter().any(|existing| existing == &dir_key) {
            keys.push(dir_key);
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    pub(crate) async fn delete_keys(&self, bucket: &str, keys: &[String]) -> Result<usize> {
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
                .await
                .map_err(|err| self.map_err(err, bucket, None))?;
            deleted += chunk.len();
        }
        Ok(deleted)
    }
}

pub(crate) trait ResponseStatus {
    fn status_code(&self) -> u16;
}

impl<B> ResponseStatus for HttpResponse<B> {
    fn status_code(&self) -> u16 {
        self.status().as_u16()
    }
}

pub(crate) fn map_s3_err<E, R>(
    err: SdkError<E, R>,
    bucket: &str,
    key: Option<&str>,
    endpoint: &str,
) -> anyhow::Error
where
    E: std::error::Error + Send + Sync + 'static,
    R: ResponseStatus,
{
    match &err {
        SdkError::ServiceError(service_err) => {
            let status = service_err.raw().status_code();
            match status {
                404 => anyhow!("remote object not found: {}", key.unwrap_or("(unknown)")),
                403 => anyhow!("access denied for bucket `{bucket}`—check keys and endpoint"),
                301 => anyhow!(
                    "region or endpoint mismatch for bucket `{bucket}`—check config endpoint/region"
                ),
                _ => anyhow!("{err}"),
            }
        }
        SdkError::DispatchFailure(_) => anyhow!("failed to connect to {endpoint}"),
        SdkError::TimeoutError(_) => anyhow!("request to {endpoint} timed out"),
        _ => anyhow!("{err}"),
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

fn merge_remote_entries(
    dir_markers: HashMap<String, String>,
    dir_names: BTreeSet<String>,
    files: Vec<RemoteEntry>,
) -> Vec<RemoteEntry> {
    let mut entries: Vec<RemoteEntry> = dir_names
        .into_iter()
        .map(|name| RemoteEntry {
            modified: dir_markers.get(&name).cloned(),
            name,
            is_dir: true,
            size: None,
        })
        .collect();
    entries.extend(files);
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.cmp(&right.name))
    });
    entries
}

pub(crate) fn normalize_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

pub(crate) fn normalize_key(key: &str) -> String {
    key.trim_start_matches('/').to_owned()
}

pub(crate) fn normalize_dir_key(key: &str) -> String {
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
    pub(crate) async fn list_keys_recursive(&self, bucket: &str, key: &str) -> Result<Vec<String>> {
        let prefix = normalize_dir_key(key);
        let mut continuation = None;
        let mut keys = Vec::new();

        loop {
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(bucket)
                .prefix(prefix.clone());
            if let Some(token) = continuation.clone() {
                request = request.continuation_token(token);
            }
            let output = request
                .send()
                .await
                .map_err(|err| self.map_err(err, bucket, Some(key)))?;
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

pub(crate) fn part_plan(total_bytes: u64, configured_part_size: u64) -> Vec<(u32, u64, u64)> {
    let part_size = effective_part_size(total_bytes, configured_part_size);
    let mut parts = Vec::new();
    let mut offset = 0u64;
    let mut part_number = 1u32;

    while offset < total_bytes {
        let remaining = total_bytes - offset;
        let len = remaining.min(part_size);
        parts.push((part_number, offset, len));
        offset += len;
        part_number += 1;
    }

    parts
}

fn effective_part_size(total_bytes: u64, configured_part_size: u64) -> u64 {
    let min_for_count = total_bytes.div_ceil(MAX_PARTS);
    let mut part_size = configured_part_size.max(min_for_count);
    part_size = part_size.div_ceil(MIB) * MIB;
    part_size.max(MIN_NON_LAST_PART_SIZE)
}

fn sort_completed_parts(parts: &mut [CompletedPart]) {
    parts.sort_by_key(|part| part.part_number().unwrap_or(0));
}

fn upload_interrupted(key: &str) -> anyhow::Error {
    io::Error::new(
        ErrorKind::Interrupted,
        format!("upload interrupted for `{key}`"),
    )
    .into()
}

struct MultipartUploadGuard {
    client: Client,
    bucket: String,
    key: String,
    upload_id: String,
    completed: bool,
    abort_attempted: bool,
    runtime_handle: tokio::runtime::Handle,
}

impl MultipartUploadGuard {
    fn new(
        client: Client,
        bucket: String,
        key: String,
        upload_id: String,
        runtime_handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            client,
            bucket,
            key,
            upload_id,
            completed: false,
            abort_attempted: false,
            runtime_handle,
        }
    }

    async fn abort(&mut self) {
        if self.completed || self.abort_attempted {
            return;
        }
        self.abort_attempted = true;
        if let Err(err) = self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(&self.key)
            .upload_id(&self.upload_id)
            .send()
            .await
        {
            eprintln!("warning: failed to abort multipart upload: {err}");
        }
    }

    fn mark_completed(&mut self) {
        self.completed = true;
    }
}

impl Drop for MultipartUploadGuard {
    fn drop(&mut self) {
        if self.completed || self.abort_attempted {
            return;
        }
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let key = self.key.clone();
        let upload_id = self.upload_id.clone();
        self.runtime_handle.spawn(async move {
            let _ = client
                .abort_multipart_upload()
                .bucket(&bucket)
                .key(&key)
                .upload_id(&upload_id)
                .send()
                .await;
        });
    }
}

pub(crate) fn download_part_path(local_path: &Path) -> PathBuf {
    let file_name = local_path
        .file_name()
        .map(|name| format!("{}.download", name.to_string_lossy()))
        .unwrap_or_else(|| ".download".to_owned());
    local_path.with_file_name(file_name)
}

pub(crate) async fn existing_part_size(part_path: &Path) -> Result<u64> {
    match tokio::fs::metadata(part_path).await {
        Ok(metadata) => Ok(metadata.len()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(0),
        Err(err) => Err(err).with_context(|| format!("failed to inspect {}", part_path.display())),
    }
}

pub(crate) async fn open_download_file(part_path: &Path, resume_from: u64) -> Result<File> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_prefix_adds_trailing_slash() {
        assert_eq!(normalize_prefix("photos"), "photos/");
        assert_eq!(normalize_prefix("/photos/"), "photos/");
        assert_eq!(normalize_prefix(""), "");
    }

    #[test]
    fn normalize_key_strips_leading_slash() {
        assert_eq!(normalize_key("/foo/bar"), "foo/bar");
        assert_eq!(normalize_key("foo"), "foo");
    }

    #[test]
    fn normalize_dir_key_ensures_trailing_slash() {
        assert_eq!(normalize_dir_key("foo"), "foo/");
        assert_eq!(normalize_dir_key("foo/"), "foo/");
        assert_eq!(normalize_dir_key(""), "");
    }

    #[test]
    fn merge_remote_entries_sorts_dirs_first() {
        let mut dir_markers = HashMap::new();
        dir_markers.insert("b".to_owned(), "Jan 1".to_owned());
        let dir_names = BTreeSet::from(["b".to_owned(), "a".to_owned()]);
        let files = vec![RemoteEntry {
            name: "c.txt".to_owned(),
            is_dir: false,
            size: Some(1),
            modified: None,
        }];
        let entries = merge_remote_entries(dir_markers, dir_names, files);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_dir);
        assert!(entries[1].is_dir);
        assert_eq!(entries[0].name, "a");
        assert_eq!(entries[1].name, "b");
        assert_eq!(entries[1].modified.as_deref(), Some("Jan 1"));
        assert_eq!(entries[2].name, "c.txt");
    }

    #[test]
    fn part_plan_covers_entire_file() {
        let total = PART_SIZE + 1;
        let parts = part_plan(total, PART_SIZE);
        let sum: u64 = parts.iter().map(|(_, _, len)| len).sum();
        assert_eq!(sum, total);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], (1, 0, PART_SIZE));
        assert_eq!(parts[1], (2, PART_SIZE, 1));
    }

    #[test]
    fn part_plan_non_last_parts_meet_minimum_size() {
        let total = PART_SIZE * 3;
        let parts = part_plan(total, PART_SIZE);
        for (idx, (_, _, len)) in parts.iter().enumerate() {
            if idx + 1 < parts.len() {
                assert!(*len >= MIN_NON_LAST_PART_SIZE);
            }
        }
    }

    #[test]
    fn part_plan_respects_max_part_count() {
        let total = PART_SIZE * (MAX_PARTS + 10);
        let parts = part_plan(total, PART_SIZE);
        assert!(parts.len() <= MAX_PARTS as usize);
        let sum: u64 = parts.iter().map(|(_, _, len)| len).sum();
        assert_eq!(sum, total);
    }

    #[test]
    fn map_s3_err_dispatch_failure_mentions_endpoint() {
        use aws_smithy_runtime_api::client::result::ConnectorError;

        let err = SdkError::<std::convert::Infallible, HttpResponse<SdkBody>>::dispatch_failure(
            ConnectorError::other("connection refused".into(), None),
        );
        let mapped = map_s3_err(err, "my-bucket", Some("key"), "https://s3.example.com");
        assert_eq!(
            mapped.to_string(),
            "failed to connect to https://s3.example.com"
        );
    }

    #[test]
    fn sort_completed_parts_orders_by_part_number() {
        let high = CompletedPart::builder()
            .part_number(3)
            .e_tag("\"c\"")
            .build();
        let low = CompletedPart::builder()
            .part_number(1)
            .e_tag("\"a\"")
            .build();
        let mid = CompletedPart::builder()
            .part_number(2)
            .e_tag("\"b\"")
            .build();
        let mut parts = vec![high, low, mid];
        sort_completed_parts(&mut parts);
        assert_eq!(parts[0].part_number(), Some(1));
        assert_eq!(parts[1].part_number(), Some(2));
        assert_eq!(parts[2].part_number(), Some(3));
    }
}
