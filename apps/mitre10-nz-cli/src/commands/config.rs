//! `config` -- read and change the settings file.

use cli_kit::table;

use crate::app::App;
use crate::cli::ConfigAction;
use crate::config::{describe, KEYS};
use crate::error::AppResult;

pub fn run(app: &App, action: ConfigAction) -> AppResult<()> {
    match action {
        ConfigAction::List => {
            let mut t = table(&["Setting", "Value", "What it is"]);
            for key in KEYS {
                t.add_row(vec![
                    key.to_string(),
                    app.config.get(key)?.unwrap_or_else(|| "—".into()),
                    describe(key).to_string(),
                ]);
            }
            println!("{t}");
            println!("{}", app.config_file.display());
        }
        ConfigAction::Get { key } => match app.config.get(&key)? {
            Some(value) => println!("{value}"),
            None => println!(),
        },
        ConfigAction::Set { key, value } => {
            let mut config = app.config.clone();
            config.set(&key, &value)?;
            app.save(&config)?;
            println!("{key} = {}", config.get(&key)?.unwrap_or_default());
        }
        ConfigAction::Unset { key } => {
            let mut config = app.config.clone();
            config.unset(&key)?;
            app.save(&config)?;
            println!("Unset {key}.");
        }
        ConfigAction::Path => {
            println!("config  {}", app.config_file.display());
            println!("state   {}", app.paths.state_dir.display());
        }
    }
    Ok(())
}
