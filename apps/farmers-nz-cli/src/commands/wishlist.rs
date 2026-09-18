//! `wishlist` -- the saved lists.
//!
//! Plural, unlike the other tools here: Farmers keeps several lists per
//! account and a plain add goes to whichever is marked preferred. There is no
//! remove, because the storefront has no call that takes one item back off a
//! list without going through the page's own form.

use cli_kit::emit;

use crate::app::App;
use crate::cli::WishlistAction;
use crate::commands::product_code;
use crate::error::AppResult;
use crate::views::WishlistView;

pub async fn run(app: &App, action: WishlistAction) -> AppResult<()> {
    match action {
        WishlistAction::List => list(app).await,
        WishlistAction::Add { code } => add(app, &code).await,
    }
}

async fn list(app: &App) -> AppResult<()> {
    let wishlists = app.client()?.wishlists().await?;
    emit(&mut app.out(), &WishlistView { wishlists })?;
    Ok(())
}

async fn add(app: &App, code: &str) -> AppResult<()> {
    let sku = product_code(code);
    app.client()?.wishlist_add(&sku).await?;
    println!("Saved {sku} to the preferred list.");
    Ok(())
}
