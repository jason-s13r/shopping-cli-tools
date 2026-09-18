//! `stock` -- which stores have a product.
//!
//! One request per region, and thirteen regions, so a bare `stock` is thirteen
//! requests against a host whose bot protection escalates on volume. That is
//! why `--region` exists and why a configured region is used when there is
//! one: the narrow answer is usually the one wanted anyway.

use cli_kit::emit;

use crate::app::App;
use crate::commands::product_code;
use crate::error::AppResult;
use crate::views::StockList;

pub async fn run(
    app: &App,
    code: &str,
    region: Option<String>,
    near: Option<String>,
    available: bool,
) -> AppResult<()> {
    let sku = product_code(code);
    let client = app.client()?;

    let mut regions = match app.region(region) {
        Some(region) => {
            let code = farmers_api::region(&region)
                .ok_or_else(|| farmers_api::Error::NoSuchRegion(region.clone()))?;
            vec![(code.to_string(), client.stock_in(&sku, code).await?)]
        }
        None => client
            .stock(&sku)
            .await?
            .into_iter()
            .map(|(code, stock)| (code.to_string(), stock))
            .collect(),
    };

    if let Some(near) = &near {
        let wanted = near.to_lowercase();
        for (_, stores) in regions.iter_mut() {
            stores.retain(|s| s.store.name.to_lowercase().contains(&wanted));
        }
    }
    if available {
        for (_, stores) in regions.iter_mut() {
            stores.retain(|s| s.available());
        }
    }
    // A region that filtered down to nothing is not a row.
    regions.retain(|(_, stores)| !stores.is_empty());

    let mut out = app.out();
    emit(
        &mut out,
        &StockList {
            sku,
            regions,
            filtered: available,
        },
    )?;
    Ok(())
}
