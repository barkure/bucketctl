use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use rustyline::{CompletionType, Config, Editor, error::ReadlineError, history::DefaultHistory};
use tokio::runtime::Runtime;

use crate::{commands, completion::ReplHelper, paths, session::Session, ui};

const HISTORY_MAX_LEN: usize = 1000;

pub(crate) fn run_repl(runtime: Arc<Runtime>, session: Arc<Mutex<Session>>) -> Result<()> {
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .completion_prompt_limit(20)
        .max_history_size(HISTORY_MAX_LEN)?
        .build();
    let mut editor = Editor::<ReplHelper, DefaultHistory>::with_config(config)?;
    editor.set_helper(Some(ReplHelper::new(runtime.clone(), session.clone())));

    let history_path = paths::history_path();
    if let Some(history_path) = history_path.as_ref() {
        let _ = paths::ensure_history_file(history_path);
        let _ = editor.load_history(history_path);
    }

    let result = repl_loop(&mut editor, &runtime, &session);

    if let Some(history_path) = history_path.as_ref()
        && editor.save_history(history_path).is_ok()
    {
        let _ = paths::restrict_private_file(history_path);
    }

    result
}

fn repl_loop(
    editor: &mut Editor<ReplHelper, DefaultHistory>,
    runtime: &Arc<Runtime>,
    session: &Arc<Mutex<Session>>,
) -> Result<()> {
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
                match commands::run_command(runtime, session, line) {
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
    args: &[String],
) -> Result<bool> {
    commands::run_noninteractive_command_line(runtime, session, args)
}
