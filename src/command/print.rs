use anyhow::Result;
use tokio::runtime::Runtime;

use crate::{s3::RemoteEntry, s3::S3Backend, ui};

pub fn print_help(mode: super::types::ExecMode) {
    match mode {
        super::types::ExecMode::Interactive => {
            println!("ls [path]");
            println!("cd [path]");
            println!("pwd");
            println!("mkdir <remote-dir>");
            println!("put [-r] <local> [remote]");
            println!("get [-r] <remote> [local]");
            println!("rm <remote>");
            println!("rm -r <remote-dir>");
            println!("rm -r -y <remote-dir>");
            println!("!<command>");
            println!("Ctrl-D");
            println!("exit");
        }
        super::types::ExecMode::NonInteractive => {
            println!("ls [<profile>[:/path]]");
            println!("mkdir <profile>:/path");
            println!("put [-r] <local> [<profile>:/path]");
            println!("get [-r] <profile>:/path [local]");
            println!("rm <profile>:/path");
            println!("rm -r -y <profile>:/path");
            println!("!<local command>");
        }
    }
}

pub fn print_remote_entries(entries: &[RemoteEntry]) {
    for entry in entries {
        if entry.is_dir {
            if let Some(modified) = entry.modified.as_deref() {
                println!(
                    "{}  {}  {}",
                    ui::stdout_dir_label("DIR"),
                    ui::stdout_time(modified),
                    ui::stdout_dir(&format!("{}/", entry.name))
                );
            } else {
                println!(
                    "{}  {}  {}",
                    ui::stdout_dir_label("DIR"),
                    ui::stdout_time(""),
                    ui::stdout_dir(&format!("{}/", entry.name))
                );
            }
        } else if let Some(size) = entry.size {
            let size = ui::stdout_size(&ui::format_bytes(size.max(0) as u64));
            if let Some(modified) = entry.modified.as_deref() {
                println!(
                    "{}  {}  {}",
                    size,
                    ui::stdout_time(modified),
                    ui::stdout_file(&entry.name)
                );
            } else {
                println!("{}  {}", size, ui::stdout_file(&entry.name));
            }
        } else {
            println!("{}", ui::stdout_file(&entry.name));
        }
    }
}

pub fn list_and_print(runtime: &Runtime, s3: &S3Backend, bucket: &str, prefix: &str) -> Result<()> {
    let entries = runtime.block_on(s3.list_prefix(bucket, prefix))?;
    print_remote_entries(&entries);
    Ok(())
}
