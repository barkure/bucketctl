use std::{
    borrow::Cow,
    env,
    io::{self, IsTerminal},
    path::Path,
    time::Duration,
};

use anyhow::Result;
use human_bytes::human_bytes;
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;

fn colors_enabled(use_stderr: bool) -> bool {
    if env::var_os("NO_COLOR").is_some() || matches!(env::var("TERM").ok().as_deref(), Some("dumb"))
    {
        return false;
    }

    if use_stderr {
        io::stderr().is_terminal()
    } else {
        io::stdout().is_terminal()
    }
}

pub fn colorize_prompt<'a>(prompt: &'a str) -> Cow<'a, str> {
    if !colors_enabled(true) {
        return Cow::Borrowed(prompt);
    }

    if prompt == "bucketctl> " {
        return Cow::Owned(format!("{}{}", "bucketctl".cyan(), "> ".dimmed()));
    }

    if let Some(prefix) = prompt.strip_suffix("> ") {
        if let Some((profile, path)) = prefix.split_once(':') {
            return Cow::Owned(format!(
                "{}:{}{}",
                profile.cyan(),
                path.blue(),
                "> ".dimmed()
            ));
        }
        return Cow::Owned(format!("{}{}", prefix.cyan(), "> ".dimmed()));
    }

    Cow::Borrowed(prompt)
}

fn status_label(text: &str, color_enabled: bool, colored: String) -> String {
    let plain = format!("{text:>11}");
    if color_enabled { colored } else { plain }
}

pub fn status_done() -> String {
    if colors_enabled(true) {
        status_label("Done", true, format!("{:>11}", "Done".green().bold()))
    } else {
        status_label("Done", false, String::new())
    }
}

pub fn status_cancelled() -> String {
    if colors_enabled(true) {
        status_label(
            "Cancelled",
            true,
            format!("{:>11}", "Cancelled".yellow().bold()),
        )
    } else {
        status_label("Cancelled", false, String::new())
    }
}

pub fn status_uploading() -> String {
    if colors_enabled(true) {
        status_label(
            "Uploading",
            true,
            format!("{:>11}", "Uploading".cyan().bold()),
        )
    } else {
        status_label("Uploading", false, String::new())
    }
}

pub fn status_downloading() -> String {
    if colors_enabled(true) {
        status_label(
            "Downloading",
            true,
            format!("{:>11}", "Downloading".cyan().bold()),
        )
    } else {
        status_label("Downloading", false, String::new())
    }
}

pub fn stdout_dir(text: &str) -> String {
    if colors_enabled(false) {
        text.blue().to_string()
    } else {
        text.to_owned()
    }
}

pub fn stdout_dir_label(text: &str) -> String {
    let plain = format!("{text:>10}");
    if colors_enabled(false) {
        plain.blue().bold().to_string()
    } else {
        plain
    }
}

pub fn stdout_size(text: &str) -> String {
    let plain = format!("{text:>10}");
    if colors_enabled(false) {
        plain.dimmed().to_string()
    } else {
        plain
    }
}

pub fn stdout_time(text: &str) -> String {
    let plain = format!("{text:<12}");
    if colors_enabled(false) {
        plain.dimmed().to_string()
    } else {
        plain
    }
}

pub fn stdout_file(text: &str) -> String {
    if is_archive_name(text) && colors_enabled(false) {
        text.magenta().to_string()
    } else {
        text.to_owned()
    }
}

pub fn stdout_profile(text: &str) -> String {
    if colors_enabled(false) {
        text.cyan().to_string()
    } else {
        text.to_owned()
    }
}

pub fn format_bytes(bytes: u64) -> String {
    human_bytes(bytes as f64)
}

fn is_archive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        ".7z", ".bz2", ".gz", ".iso", ".rar", ".tar", ".tgz", ".txz", ".xz", ".zip", ".zst",
        ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

pub(crate) fn transfer_progress_bar(total_bytes: u64) -> Result<ProgressBar> {
    let progress = ProgressBar::new(total_bytes);
    let style = ProgressStyle::with_template(
        "{spinner:.cyan} [{elapsed_precise}] [{bar:28.cyan/blue}] {bytes:>8}/{total_bytes:8} ({eta})",
    )?
    .progress_chars("#>-");
    progress.set_style(style);
    progress.enable_steady_tick(Duration::from_millis(120));
    Ok(progress)
}

pub(crate) fn print_download_cancelled(
    progress: &ProgressBar,
    key: &str,
    local_path: &Path,
    downloaded: u64,
    total_bytes: Option<u64>,
    part_path: &Path,
) -> Result<()> {
    progress.finish_and_clear();
    let progress_str = match total_bytes {
        Some(total) if total > 0 => format!(
            "{} / {} {:.1}%",
            format_bytes(downloaded),
            format_bytes(total),
            downloaded as f64 / total as f64 * 100.0
        ),
        _ => format_bytes(downloaded),
    };
    eprintln!(
        "{} {key} -> {} [{progress_str}]",
        status_cancelled(),
        local_path.display(),
    );
    eprintln!("{:>11} partial saved at {}", "", part_path.display());
    Ok(())
}

pub(crate) fn print_download_done(
    progress: &ProgressBar,
    key: &str,
    local_path: &Path,
    downloaded: u64,
    total_bytes: Option<u64>,
) -> Result<()> {
    progress.finish_and_clear();
    let progress_str = match total_bytes {
        Some(total) if total > 0 => format!(
            "{} / {} {:.1}%",
            format_bytes(downloaded),
            format_bytes(total),
            downloaded as f64 / total as f64 * 100.0
        ),
        _ => format_bytes(downloaded),
    };
    eprintln!(
        "{} {key} -> {} [{progress_str}]",
        status_done(),
        local_path.display()
    );
    Ok(())
}

pub(crate) fn print_upload_done(
    progress: &ProgressBar,
    key: &str,
    local_path: &Path,
    uploaded: u64,
    total_bytes: u64,
) -> Result<()> {
    progress.finish_and_clear();
    eprintln!(
        "{} {} -> {key} [{} / {} {:.1}%]",
        status_done(),
        local_path.display(),
        format_bytes(uploaded),
        format_bytes(total_bytes),
        if total_bytes == 0 {
            100.0
        } else {
            uploaded as f64 / total_bytes as f64 * 100.0
        }
    );
    Ok(())
}

pub(crate) fn print_upload_cancelled(
    progress: &ProgressBar,
    key: &str,
    local_path: &Path,
    uploaded: u64,
    total_bytes: u64,
) -> Result<()> {
    progress.finish_and_clear();
    eprintln!(
        "{} upload {} -> {key} [{} / {} {:.1}%]",
        status_cancelled(),
        local_path.display(),
        format_bytes(uploaded),
        format_bytes(total_bytes),
        if total_bytes == 0 {
            100.0
        } else {
            uploaded as f64 / total_bytes as f64 * 100.0
        }
    );
    Ok(())
}
