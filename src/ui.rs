use std::{
    borrow::Cow,
    env,
    io::{self, IsTerminal},
};

use human_bytes::human_bytes;
use owo_colors::OwoColorize;

fn colors_enabled(use_stderr: bool) -> bool {
    if env::var_os("NO_COLOR").is_some()
        || matches!(env::var("TERM").ok().as_deref(), Some("dumb"))
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
            return Cow::Owned(format!("{}:{}{}", profile.cyan(), path.blue(), "> ".dimmed()));
        }
        return Cow::Owned(format!("{}{}", prefix.cyan(), "> ".dimmed()));
    }

    Cow::Borrowed(prompt)
}

fn status_label(text: &str, color_enabled: bool, colored: String) -> String {
    let plain = format!("{text:>11}");
    if color_enabled {
        colored
    } else {
        plain
    }
}

pub fn status_done() -> String {
    if colors_enabled(true) {
        status_label(
            "Done",
            true,
            format!("{:>11}", "Done".green().bold()),
        )
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
