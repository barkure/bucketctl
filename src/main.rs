mod cli;
mod config;
mod repl;
mod s3;
mod session;
mod ui;

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
    let config = AppConfig::load()?;
    let profiles = config.profiles.clone().into_iter().collect();
    let args = cli.args;

    if args.len() == 1 && matches!(args[0].as_str(), "-v" | "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

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
