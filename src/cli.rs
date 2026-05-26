use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "S3 shell with an SFTP-like workflow")]
#[command(override_usage = "bucketctl [PROFILE]\n       bucketctl <COMMAND> ...")]
pub struct Cli {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}
