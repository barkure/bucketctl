use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::runtime::Runtime;

use crate::command::{
    Command, ExecMode, ExecOutcome, execute, parse_command_argv, parse_command_line,
};
use crate::session::Session;

pub(crate) fn run_command(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    line: &str,
) -> Result<bool> {
    let Some(command) = parse_command_line(line, ExecMode::Interactive)? else {
        return Ok(false);
    };
    dispatch(runtime, session, ExecMode::Interactive, command)
}

pub(crate) fn run_noninteractive_command_line(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    args: &[String],
) -> Result<bool> {
    let Some(command) = parse_command_argv(args, ExecMode::NonInteractive)? else {
        return Ok(false);
    };
    dispatch(runtime, session, ExecMode::NonInteractive, command)
}

fn dispatch(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    mode: ExecMode,
    command: Command,
) -> Result<bool> {
    match execute(runtime, session, mode, command)? {
        ExecOutcome::Continue => Ok(false),
        ExecOutcome::ExitRepl => Ok(true),
        ExecOutcome::ExitCode(code) => std::process::exit(code),
    }
}

pub(crate) use crate::session::with_session;
