//! `wishlist` -- saved items, which need an account.

use cli_kit::emit;

use crate::app::App;
use crate::cli::WishlistAction;
use crate::error::AppResult;
use crate::views::WishlistView;

pub async fn run(app: &App, action: WishlistAction) -> AppResult<()> {
    let client = app.client()?;
    match action {
        WishlistAction::List => {
            let wishlist = client.wishlist().await?;
            let mut out = app.out();
            emit(
                &mut out,
                &WishlistView {
                    wishlist: &wishlist,
                },
            )?;
        }
        WishlistAction::Add { code } => {
            let code = super::product_code(&code);
            client.wishlist_add(&code).await?;
            println!("Saved {code}.");
        }
        WishlistAction::Clear => {
            client.wishlist_clear().await?;
            println!("Cleared.");
        }
    }
    Ok(())
}
