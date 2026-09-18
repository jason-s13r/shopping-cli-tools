//! `orders` -- what has been bought.

use cli_kit::emit;

use crate::app::App;
use crate::error::AppResult;
use crate::views::OrderList;

pub async fn run(app: &App) -> AppResult<()> {
    let orders = app.client()?.orders().await?;
    emit(&mut app.out(), &OrderList { orders })?;
    Ok(())
}
