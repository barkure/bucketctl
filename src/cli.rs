use std::path::PathBuf;

use clap::{ArgAction, Parser};

#[derive(Debug, Parser)]
#[command(about = "S3 shell with an SFTP-like workflow")]
#[command(disable_help_flag = true)]
#[command(disable_version_flag = true)]
#[command(
    override_usage = "bucketctl [-c <PATH>] [PROFILE]\n       bucketctl [-c <PATH>] <COMMAND> ..."
)]
#[command(
    help_template = "{about-with-newline}\nUsage: {usage}\n\nOptions:\n{options}\n\nExamples:\n  bucketctl ls\n  bucketctl ls mybucket:/path\n  bucketctl put ./local.txt mybucket:/path\n  bucketctl get mybucket:/path/file ./file\n  bucketctl mybucket\n"
)]
pub struct Cli {
    #[arg(short, long, value_name = "PATH", help = "Use the given config.toml")]
    pub config: Option<PathBuf>,

    #[arg(short = 'v', long = "version", action = ArgAction::SetTrue, help = "Print version")]
    pub version: bool,

    #[arg(short = 'h', long = "help", action = ArgAction::SetTrue, help = "Show help")]
    pub help: bool,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}
