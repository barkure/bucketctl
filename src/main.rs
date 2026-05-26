mod cli;
mod config;
mod repl;
mod s3;
mod session;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use cli::Cli;
use config::AppConfig;
use repl::run_repl;
use s3::S3Backend;
use session::Session;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut args = cli.args;

    if args.len() == 1 && matches!(args[0].as_str(), "-v" | "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let config_path = extract_config_path(&mut args)?;
    let config = AppConfig::load(config_path.as_deref())?;
    let profiles = config.profiles.clone().into_iter().collect();

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    let session = Session::new(profiles);
    let session = Arc::new(Mutex::new(session));

    if args.is_empty() {
        repl::run_noninteractive_command_line(&runtime, &session, "ls")?;
        Ok(())
    } else if looks_like_command(&args[0]) {
        repl::run_noninteractive_command_line(&runtime, &session, &args.join(" "))?;
        Ok(())
    } else if args.len() == 1 {
        let profile_name = args[0].clone();
        let profile = config.profile(&profile_name)?.clone();
        let s3 = runtime.block_on(S3Backend::connect(&profile))?;
        {
            let mut guard = session
                .lock()
                .map_err(|_| anyhow::anyhow!("session lock poisoned"))?;
            guard.attach_profile(profile_name, profile.bucket.clone(), s3);
        }
        run_repl(runtime, session)
    } else {
        anyhow::bail!(
            "non-interactive mode uses command-first syntax, for example `bucketctl ls mybucket`"
        )
    }
}

fn looks_like_command(token: &str) -> bool {
    matches!(
        token,
        "help"
            | "ls"
            | "mkdir"
            | "put"
            | "get"
            | "rm"
    ) || token.starts_with('!')
}

fn extract_config_path(args: &mut Vec<String>) -> Result<Option<PathBuf>> {
    let mut path: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].clone();
        if arg == "-c" || arg == "--config" {
            if i + 1 >= args.len() {
                anyhow::bail!("`{arg}` requires a path argument");
            }
            path = Some(PathBuf::from(args[i + 1].clone()));
            args.drain(i..=i + 1);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--config=") {
            if value.is_empty() {
                anyhow::bail!("`--config=` requires a path value");
            }
            path = Some(PathBuf::from(value));
            args.remove(i);
            continue;
        }
        i += 1;
    }
    Ok(path)
}
