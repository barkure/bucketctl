use std::io::{self, Write};

use anyhow::{Result, bail};

const COMMANDS: &[&str] = &[
    "help",
    "ls",
    "cd",
    "pwd",
    "mkdir",
    "put",
    "get",
    "rm",
    "exit",
    "completion",
    "config",
];

pub fn emit(shell: &str, profiles: &[String]) -> Result<()> {
    let script = match shell {
        "bash" => bash_script(profiles),
        "zsh" => zsh_script(profiles),
        "fish" => fish_script(profiles),
        other => bail!("unsupported shell `{other}`; expected bash, zsh, or fish"),
    };
    io::stdout()
        .write_all(script.as_bytes())
        .map_err(|err| anyhow::anyhow!(err))?;
    Ok(())
}

fn bash_script(profiles: &[String]) -> String {
    let commands = COMMANDS.join(" ");
    let profile_words: String = profiles
        .iter()
        .map(|profile| format!("\"{profile}\""))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        r#"_bucketctl_completion() {{
    local cur prev words cword
    _init_completion || return

    local commands="{commands}"
    local profiles=({profile_words})

    if (( cword == 1 )); then
        COMPREPLY=( $(compgen -W "$commands ${{profiles[@]}}" -- "$cur") )
        return
    fi

    case "${{words[1]}}" in
        ls|mkdir|put|get|rm|cd)
            COMPREPLY=( $(compgen -f -- "$cur") )
            ;;
    esac
}}

complete -F _bucketctl_completion bucketctl bkt
"#
    )
}

fn zsh_script(profiles: &[String]) -> String {
    let commands = COMMANDS.join(" ");
    let profile_words: String = profiles
        .iter()
        .map(|profile| format!("'{profile}'"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        r#"#compdef bucketctl bkt

_bucketctl() {{
    local -a commands profiles
    commands=({commands})
    profiles=({profile_words})

    if (( CURRENT == 2 )); then
        _describe 'command/profile' commands profiles
        return
    fi

    case $words[2] in
        ls|mkdir|put|get|rm|cd)
            _files
            ;;
    esac
}}

_bucketctl "$@"
"#
    )
}

fn fish_script(profiles: &[String]) -> String {
    let mut script = String::from("complete -c bucketctl -c bkt\n");
    for command in COMMANDS {
        script.push_str(&format!(
            "complete -c bucketctl -c bkt -n '__fish_use_subcommand' -a '{command}'\n"
        ));
    }
    for profile in profiles {
        script.push_str(&format!(
            "complete -c bucketctl -c bkt -n '__fish_use_subcommand' -a '{profile}'\n"
        ));
    }
    script.push_str(
        "complete -c bucketctl -c bkt -n '__fish_seen_subcommand_from ls mkdir put get rm cd' -f\n",
    );
    script
}
