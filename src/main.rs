mod cli;
mod command;
mod commands;
mod completion;
mod config;
mod dispatch;
mod paths;
mod repl;
mod s3;
mod session;
mod shell_complete;
mod ui;

use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use clap::{CommandFactory, Parser};
use cli::Cli;
use config::AppConfig;
use repl::run_repl;
use s3::S3Backend;
use session::Session;

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.help {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    }
    if cli.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let args = cli.args;
    if let Some(shell) = args.first().map(String::as_str) {
        if shell == "completion" {
            let profiles = config::load_profiles_optional(cli.config.as_deref())?;
            return shell_complete::emit(args.get(1).map(String::as_str).unwrap_or(""), &profiles);
        }
        if shell == "config" {
            return handle_config_subcommand(&args[1..], cli.config.as_deref());
        }
    }

    let config = AppConfig::load(cli.config.as_deref())?;
    let profiles = config.profiles.clone().into_iter().collect();
    let default_profile = config.default_profile().map(ToOwned::to_owned);

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    let session = Session::new(profiles, default_profile);
    let session = Arc::new(Mutex::new(session));

    if args.is_empty() {
        let profiles = {
            let guard = session
                .lock()
                .map_err(|_| anyhow::anyhow!("session lock poisoned"))?;
            guard.list_profiles()
        };
        if !profiles.is_empty() {
            let rendered = profiles
                .into_iter()
                .map(|p| ui::stdout_profile(&p))
                .collect::<Vec<_>>()
                .join("  ");
            println!("{rendered}");
        }
        return Ok(());
    }

    if dispatch::is_command_token(&args[0]) {
        if let Some(profile_name) = config.default_profile() {
            let profile = config.profile(profile_name)?.clone();
            let s3 = runtime.block_on(S3Backend::connect(&profile))?;
            {
                let mut guard = session
                    .lock()
                    .map_err(|_| anyhow::anyhow!("session lock poisoned"))?;
                guard.attach_profile(profile_name.to_owned(), profile.bucket.clone(), s3);
            }
        }
        repl::run_noninteractive_command_line(&runtime, &session, &args)?;
        return Ok(());
    }

    if args.len() == 1 {
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
        bail!("non-interactive mode uses command-first syntax, for example `bucketctl ls mybucket`")
    }
}

fn handle_config_subcommand(
    args: &[String],
    override_path: Option<&std::path::Path>,
) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("init") => {
            let force = args.iter().any(|arg| arg == "--force");
            config::init_config(override_path, force)
        }
        Some(other) => bail!("unknown config subcommand `{other}`; try `bucketctl config init`"),
        None => bail!("usage: bucketctl config init [--force]"),
    }
}
