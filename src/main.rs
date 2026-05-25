mod cli;
mod config;
mod repl;
mod s3;
mod session;

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
    let (profile_name, command) = normalize_cli(cli, &config);

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    let mut session = Session::new(profiles);
    if let Some(profile_name) = profile_name {
        let profile = config.profile(&profile_name)?.clone();
        let s3 = runtime.block_on(S3Backend::connect(&profile))?;
        session.attach_profile(profile_name, profile.bucket.clone(), s3);
    }
    let session = Arc::new(Mutex::new(session));

    if command.is_empty() {
        run_repl(runtime, session)
    } else {
        repl::run_command_line(&runtime, &session, &command.join(" "))?;
        Ok(())
    }
}

fn normalize_cli(cli: Cli, config: &AppConfig) -> (Option<String>, Vec<String>) {
    match cli.profile {
        Some(first)
            if !config.profiles.contains_key(&first) && looks_like_command(&first) =>
        {
            let mut command = vec![first];
            command.extend(cli.command);
            (None, command)
        }
        profile => (profile, cli.command),
    }
}

fn looks_like_command(token: &str) -> bool {
    matches!(
        token,
        "help"
            | "ls"
            | "attach"
            | "cd"
            | "pwd"
            | "mkdir"
            | "put"
            | "get"
            | "rm"
            | "exit"
    ) || token.starts_with('!')
}
