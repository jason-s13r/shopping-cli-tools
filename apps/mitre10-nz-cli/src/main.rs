//! `mitre10` -- Mitre 10 New Zealand from the command line.
//!
//! The interesting code is in `packages/`: the storefront protocol in
//! `mitre10-api`, the rendering in `cli-kit`. What is left here is the part that
//! is genuinely about this program -- reading the environment once, resolving
//! flags against config, and turning a failure into an exit code.

mod app;
mod build;
mod cli;
mod commands;
mod config;
mod env;
mod error;
mod views;

use std::process::ExitCode;

use clap::Parser;

use crate::app::App;
use crate::cli::Cli;
use crate::error::AppResult;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("mitre10: could not start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.silent() => ExitCode::from(e.exit_code()),
        Err(e) => {
            eprintln!("mitre10: {e}");
            // The chain, not just the top: the cause underneath is the part
            // worth reading. Skipping anything the line above already said,
            // because a wrapper carrying its source's own words would print
            // them twice and read as two problems.
            let mut shown = e.to_string();
            let mut cause = std::error::Error::source(&e);
            while let Some(e) = cause {
                let text = e.to_string();
                if !shown.contains(&text) {
                    eprintln!("      {text}");
                    shown = text;
                }
                cause = e.source();
            }
            // The library says what is wrong and, separately, what kind of
            // thing would fix it. Only this binary knows it is called `mitre10`,
            // so turning that into a command line happens here.
            if let Some(advice) = cli::advice(&e) {
                eprintln!("      {advice}");
            }
            // 2 misuse, 3 auth, 5 no store, 7 rate limited -- so a script can
            // tell them apart without reading this text.
            ExitCode::from(e.exit_code())
        }
    }
}

async fn run(cli: Cli) -> AppResult<()> {
    let app = App::new(&cli)?;
    commands::run(&app, cli.command).await
}
