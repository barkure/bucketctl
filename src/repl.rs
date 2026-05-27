use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use rustyline::{
    CompletionType, Config,
    Editor,
    error::ReadlineError,
    history::DefaultHistory,
};
use tokio::runtime::Runtime;

use crate::{
    commands,
    completion::ReplHelper,
    session::Session,
    ui,
};

pub(crate) fn run_repl(runtime: Arc<Runtime>, session: Arc<Mutex<Session>>) -> Result<()> {
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .completion_prompt_limit(20)
        .build();
    let mut editor = Editor::<ReplHelper, DefaultHistory>::with_config(config)?;
    editor.set_helper(Some(ReplHelper::new(runtime.clone(), session.clone())));

    loop {
        let prompt = {
            let guard = session
                .lock()
                .map_err(|_| anyhow!("session lock poisoned"))?;
            guard.prompt()
        };
        if let Some(helper) = editor.helper_mut() {
            helper.colored_prompt = ui::colorize_prompt(&prompt).into_owned();
        }

        match editor.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);
                match commands::run_command(&runtime, &session, line) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(err) => eprintln!("Error: {err:#}"),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(err) => return Err(err.into()),
        }
    }

    Ok(())
}

pub(crate) fn run_noninteractive_command_line(
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
    line: &str,
) -> Result<bool> {
    commands::run_noninteractive_command_line(runtime, session, line)
}
