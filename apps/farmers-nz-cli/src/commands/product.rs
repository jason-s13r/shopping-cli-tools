//! `product`, `variants` and `prices`.

use cli_kit::emit;

use crate::app::App;
use crate::commands::{codes_from_stdin, product_code};
use crate::error::AppResult;
use crate::views::{PriceList, ProductView, VariantList};

pub async fn show(app: &App, code: &str, want_variants: bool) -> AppResult<()> {
    let sku = product_code(code);
    let client = app.client()?;
    let product = client.product(&sku).await?;

    // Fetched unasked for a master, because its own record has no stock and a
    // range for a price -- so without them the answer is "you cannot buy this"
    // and nothing else.
    let variants = match want_variants || product.master {
        false => None,
        true => Some(client.variants(&sku).await?),
    };

    let mut out = app.out();
    emit(&mut out, &ProductView { product, variants })?;
    Ok(())
}

pub async fn variants(app: &App, code: &str) -> AppResult<()> {
    let sku = product_code(code);
    let variants = app.client()?.variants(&sku).await?;
    let mut out = app.out();
    emit(&mut out, &VariantList { sku, variants })?;
    Ok(())
}

pub async fn prices(app: &App, codes: Vec<String>) -> AppResult<()> {
    let codes: Vec<String> = match codes.is_empty() {
        true => codes_from_stdin(),
        false => codes.iter().map(|c| product_code(c)).collect(),
    };
    if codes.is_empty() {
        // Not an error: `farmers prices < empty-file` asking for nothing and
        // getting nothing is a reasonable thing to have happened.
        println!("No product codes given.");
        return Ok(());
    }

    let prices = app
        .client()?
        .products(&codes)
        .await
        .into_iter()
        // One failure does not stop the rest: "which of these forty is gone"
        // is the question a batch is asked.
        .map(|(code, result)| (code, result.map_err(|e| e.to_string())))
        .collect();

    let mut out = app.out();
    emit(&mut out, &PriceList { prices })?;
    Ok(())
}
