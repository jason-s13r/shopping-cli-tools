//! One module per command. Each takes the assembled [`App`] and the flags it
//! was given, and nothing else: no command reads the environment, opens the
//! config file or decides how to render.

pub mod auth;
pub mod cart;
pub mod categories;
pub mod completions;
pub mod config;
pub mod doctor;
pub mod listing;
pub mod orders;
pub mod product;
pub mod regions;
pub mod stock;
pub mod update;
pub mod wishlist;

use crate::app::App;
use crate::cli::Command;
use crate::error::AppResult;

pub async fn run(app: &App, command: Command) -> AppResult<()> {
    match command {
        Command::Search { query, listing } => listing::search(app, &query, listing).await,
        Command::Browse { category, listing } => listing::browse(app, &category, listing).await,
        Command::Suggest { term, limit } => listing::suggest(app, &term, limit).await,
        Command::Categories {
            query,
            depth,
            brands,
        } => categories::run(app, query, depth, brands).await,
        Command::Product { code, variants } => product::show(app, &code, variants).await,
        Command::Variants { code } => product::variants(app, &code).await,
        Command::Stock {
            code,
            region,
            near,
            available,
        } => stock::run(app, &code, region, near, available).await,
        Command::Prices { codes } => product::prices(app, codes).await,
        Command::Regions => regions::run(app),
        Command::Cart { action } => cart::run(app, action).await,
        Command::Wishlist { action } => wishlist::run(app, action).await,
        Command::Orders => orders::run(app).await,
        Command::Auth { action } => auth::run(app, action).await,
        Command::Config { action } => config::run(app, action),
        Command::Doctor => doctor::run(app).await,
        Command::Update {
            version,
            check,
            pre_release,
        } => update::run(app, version, check, pre_release).await,
        Command::Completions { shell } => completions::run(app, shell),
    }
}

/// A product code, however it was given.
///
/// The site's product URLs end in the code, sometimes followed by every
/// variant code `|`-separated -- `…/chisel-fleece-robe-charcoal-6867065|6867065001|6867065002`
/// is what the search index hands back and what a browser's address bar holds.
/// Both are what someone reaching for this actually has.
pub fn product_code(input: &str) -> String {
    let trimmed = input.trim();
    // The master is the first of the `|` list, which is the one a product page
    // opens on.
    let trimmed = trimmed.split('|').next().unwrap_or(trimmed);
    let last = trimmed
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(trimmed);
    // A slug ends in the code after a hyphen; a bare code has no hyphen to
    // split on and comes through whole.
    match last.rsplit('-').next() {
        Some(code) if !code.is_empty() && code.chars().all(|c| c.is_ascii_digit()) => {
            code.to_string()
        }
        _ => last.to_string(),
    }
}

/// Read codes from stdin, one per line, for a batch given none on the command
/// line.
pub fn codes_from_stdin() -> Vec<String> {
    use std::io::BufRead;
    std::io::stdin()
        .lock()
        .lines()
        .map_while(Result::ok)
        .map(|line| product_code(&line))
        .filter(|code| !code.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_code_comes_through_untouched() {
        assert_eq!(product_code("6867065002"), "6867065002");
        assert_eq!(product_code("  6867065002 "), "6867065002");
    }

    #[test]
    fn a_pasted_product_url_gives_up_its_code() {
        assert_eq!(
            product_code("https://www.farmers.co.nz/men/robes/chisel-fleece-robe-charcoal-6867065"),
            "6867065"
        );
        assert_eq!(product_code("/men/robes/a-thing-1234567"), "1234567");
    }

    #[test]
    fn the_variant_list_the_index_appends_resolves_to_the_master() {
        // The search index's `url` carries every variant after a `|`, and that
        // whole string is what ends up pasted. The master is the first.
        assert_eq!(
            product_code("men/robes/chisel-fleece-robe-charcoal-6867065|6867065001|6867065002"),
            "6867065"
        );
    }

    #[test]
    fn a_slug_with_no_trailing_code_is_left_alone_rather_than_mangled() {
        // Better to send it and be told "no such product" than to invent a
        // code out of the last word of a slug.
        assert_eq!(
            product_code("/men/sleepwear-robes-slippers"),
            "sleepwear-robes-slippers"
        );
    }
}
