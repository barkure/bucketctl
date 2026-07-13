use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Exit,
    Pwd,
    Ls {
        target: Option<RemoteSpec>,
    },
    Cd {
        target: String,
    },
    Mkdir {
        target: RemoteSpec,
    },
    Put {
        local: PathBuf,
        remote: Option<RemoteSpec>,
        recursive: bool,
    },
    Get {
        remote: RemoteSpec,
        local: Option<PathBuf>,
        recursive: bool,
    },
    Rm {
        remote: RemoteSpec,
        recursive: bool,
        yes: bool,
    },
    Shell {
        command: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteSpec {
    Path(String),
    ProfilePath {
        profile: String,
        path: String,
    },
    /// Noninteractive `ls <name>` when the token has no `:` — resolved at exec time.
    BareLsTarget(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    Interactive,
    NonInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecOutcome {
    Continue,
    ExitRepl,
    ExitCode(i32),
}
