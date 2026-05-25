use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "S3 shell with an SFTP-like workflow")]
pub struct Cli {
    pub profile: Option<String>,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}
