//! `categories` -- the category tree.

use cli_kit::emit;

use crate::app::App;
use crate::error::AppResult;
use crate::views::CategoryTree;

/// The branch the catalogue files brands under. Left out unless asked for: it
/// has 1,354 children and would bury the eleven real ones.
const BRANDS: &str = "Brands";

pub async fn run(app: &App, query: Option<String>, depth: usize, brands: bool) -> AppResult<()> {
    let mut categories = app.client()?.categories(depth).await?;
    if !brands {
        categories.retain(|c| c.id != BRANDS);
    }

    let next = (!brands).then(|| {
        "Browse one with `farmers browse <id>`. Brands are hidden; \
         `farmers categories --brands` includes them."
            .to_string()
    });

    let mut out = app.out();
    emit(
        &mut out,
        &CategoryTree {
            categories,
            query,
            next,
        },
    )?;
    Ok(())
}
