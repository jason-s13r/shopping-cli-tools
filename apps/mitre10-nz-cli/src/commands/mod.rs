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
pub mod stores;
pub mod update;
pub mod wishlist;

use crate::app::App;
use crate::cli::Command;
use crate::error::AppResult;

pub async fn run(app: &App, command: Command) -> AppResult<()> {
    match command {
        Command::Search { query, listing } => listing::search(app, query, listing).await,
        Command::Browse { category, listing } => listing::browse(app, category, listing).await,
        Command::Suggest { term, limit } => listing::suggest(app, &term, limit).await,
        Command::Categories { query, depth } => categories::run(app, query, depth).await,
        Command::Product { code, store } => product::show(app, &code, store).await,
        Command::Stock {
            code,
            near,
            available,
        } => product::stock(app, &code, near, available).await,
        Command::Prices { codes, store } => product::prices(app, codes, store).await,
        Command::Stores {
            query,
            postcode,
            near,
        } => stores::list(app, query, postcode, near).await,
        Command::Store { action } => stores::store(app, action).await,
        Command::Cart { action } => cart::run(app, action).await,
        Command::Wishlist { action } => wishlist::run(app, action).await,
        Command::Orders { page, limit } => orders::run(app, page, limit).await,
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
/// The site's URLs end in the code, so a pasted product link works where a
/// bare code does -- which is what someone reaching for this actually has.
pub fn product_code(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.contains('/') {
        if let Some(code) = mitre10_api::wire::code_from_url(trimmed) {
            return code;
        }
    }
    trimmed.trim_start_matches('0').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_product_url_is_as_good_as_a_code() {
        assert_eq!(
            product_code("https://www.mitre10.co.nz/shop/wattyl-fence-finish/p/174969"),
            "174969"
        );
        assert_eq!(product_code("/shop/a-thing/p/269938"), "269938");
        assert_eq!(product_code("174969"), "174969");
        assert_eq!(product_code("  174969 "), "174969");
    }

    #[test]
    fn a_padded_code_is_unpadded_because_that_is_what_most_calls_take() {
        // The batch endpoint pads; everything else refuses the padded form.
        assert_eq!(product_code("000000000000174969"), "174969");
    }
}
