//! `orders` -- what has been bought.

use cli_kit::emit;

use crate::app::App;
use crate::error::AppResult;
use crate::views::OrderList;

pub async fn run(app: &App, page: u64, limit: u64) -> AppResult<()> {
    let orders = app.client()?.orders(page, limit).await?;
    let mut out = app.out();
    emit(&mut out, &OrderList { page: &orders })?;
    Ok(())
}
