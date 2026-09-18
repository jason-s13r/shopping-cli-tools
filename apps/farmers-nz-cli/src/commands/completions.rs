//! `completions` -- the shell script, and nothing else on the stream, so
//! `source <(farmers completions zsh)` works.

use crate::app::App;
use crate::error::{AppError, AppResult};

pub fn run(app: &App, shell: Option<String>) -> AppResult<()> {
    let shell = match shell {
        Some(name) => Some(name.parse::<clap_complete::Shell>().map_err(|_| {
            AppError::usage(format!(
                "{name:?} is not a shell; use bash, zsh, fish, elvish or powershell"
            ))
        })?),
        None => None,
    };
    let mut out = std::io::stdout();
    cli_kit::completions::generate(
        &mut crate::cli::command(),
        "farmers",
        shell,
        app.env.shell.as_deref(),
        &mut out,
    )
    .map_err(AppError::usage)
}
