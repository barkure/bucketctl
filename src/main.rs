mod cdn;
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
mod ui;

use std::io::{self, IsTerminal};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use cdn::CdnBackend;
use clap::{CommandFactory, Parser};
use cli::Cli;
use config::AppConfig;
use dialoguer::Select;
use repl::run_repl;
use s3::S3Backend;
use session::Session;
use tokio::runtime::Runtime;

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

    if let Some(first) = args.first().map(String::as_str)
        && first == "init"
    {
        let force = args.iter().any(|arg| arg == "--force");
        return config::init_config(cli.config.as_deref(), force);
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
        if profiles.is_empty() {
            return Ok(());
        }
        if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
            let rendered = profiles
                .iter()
                .map(|p| ui::stdout_profile(p))
                .collect::<Vec<_>>()
                .join("  ");
            println!("{rendered}");
            return Ok(());
        }
        let index = match Select::new()
            .with_prompt("select a profile")
            .items(&profiles)
            .default(0)
            .interact_opt()
        {
            Ok(Some(index)) => index,
            Ok(None) => return Ok(()),
            Err(dialoguer::Error::IO(err)) if err.kind() == io::ErrorKind::Interrupted => {
                return Ok(());
            }
            Err(err) => return Err(err.into()),
        };
        return enter_profile_repl(&runtime, &session, &config, &profiles[index]);
    }

    if dispatch::is_command_token(&args[0]) {
        if let Some(profile_name) = config.default_profile() {
            let profile = config.profile(profile_name)?.clone();
            let s3 = runtime.block_on(S3Backend::connect(&profile))?;
            let cdn = CdnBackend::from_profile(&profile)?;
            {
                let mut guard = session
                    .lock()
                    .map_err(|_| anyhow::anyhow!("session lock poisoned"))?;
                guard.attach_profile(profile_name.to_owned(), profile.bucket.clone(), s3, cdn);
            }
        }
        repl::run_noninteractive_command_line(&runtime, &session, &args)?;
        return Ok(());
    }

    bail!("non-interactive mode uses command-first syntax, for example `bucketctl ls mybucket`")
}

fn enter_profile_repl(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    config: &AppConfig,
    profile_name: &str,
) -> Result<()> {
    let profile = config.profile(profile_name)?.clone();
    let s3 = runtime.block_on(S3Backend::connect(&profile))?;
    let cdn = CdnBackend::from_profile(&profile)?;
    {
        let mut guard = session
            .lock()
            .map_err(|_| anyhow::anyhow!("session lock poisoned"))?;
        guard.attach_profile(profile_name.to_owned(), profile.bucket.clone(), s3, cdn);
    }
    run_repl(runtime.clone(), session.clone())
}
