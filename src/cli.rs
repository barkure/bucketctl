use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "S3 shell with an SFTP-like workflow")]
#[command(override_usage = "bucketctl [-c <PATH>] [PROFILE]\n       bucketctl [-c <PATH>] <COMMAND> ...")]
#[command(
    after_help = "Options:\n  -c, --config <PATH>  Use the given config.toml instead of ~/.config/bucketctl/config.toml"
)]
pub struct Cli {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}
