//! `product`, `stock` and `prices` -- the catalogue, one product or many.

use std::io::{BufRead, Write};

use cli_kit::{emit, table};

use crate::app::App;
use crate::error::{AppError, AppResult};
use crate::views::{ProductView, StockList};

pub async fn show(app: &App, code: &str, store: Option<String>) -> AppResult<()> {
    let code = super::product_code(code);
    let store = app.store(store);
    let product = app.client()?.product(&code, store.as_deref()).await?;
    let mut out = app.out();
    emit(&mut out, &ProductView { product: &product })?;
    Ok(())
}

pub async fn stock(app: &App, code: &str, near: Option<String>, available: bool) -> AppResult<()> {
    let code = super::product_code(code);
    let mut stock = app.client()?.stock(&code).await?;

    if let Some(near) = &near {
        let needle = near.to_lowercase();
        stock.retain(|s| s.store_name.to_lowercase().contains(&needle));
    }
    if available {
        stock.retain(|s| s.level.is_available());
    }
    // The configured store first: it is the one the person asking almost
    // always means, and the list is otherwise in the storefront's own order.
    if let Some(mine) = app.store(None) {
        stock.sort_by_key(|s| s.store != mine);
    }

    let mut out = app.out();
    emit(
        &mut out,
        &StockList {
            code: &code,
            stock: &stock,
        },
    )?;
    Ok(())
}

/// Price a list of products in one go, reading stdin when no codes are given
/// so this composes with whatever produced the list.
pub async fn prices(app: &App, codes: Vec<String>, store: Option<String>) -> AppResult<()> {
    let codes: Vec<String> = if codes.is_empty() {
        std::io::stdin()
            .lock()
            .lines()
            .map_while(Result::ok)
            .map(|l| super::product_code(&l))
            .filter(|c| !c.is_empty())
            .collect()
    } else {
        codes.iter().map(|c| super::product_code(c)).collect()
    };

    if codes.is_empty() {
        return Err(AppError::usage(
            "no product codes; give them as arguments or on stdin, one per line",
        ));
    }

    let store = app.store(store);
    let postcode = app.postcode(None);
    let products = app
        .client()?
        .products(&codes, store.as_deref(), postcode.as_deref())
        .await?;

    let mut out = app.out();
    if out.is_json() {
        emit(
            &mut out,
            &Prices {
                products: &products,
            },
        )?;
        return Ok(());
    }

    // Code, price and size and nothing else: `priceForProducts` carries no
    // name and no stock even at fields=FULL, so promising those columns would
    // print a table of dashes. `mitre10 product` is the call that has them.
    let mut t = table(&["Code", "Price", "Was", "Size"]);
    for p in &products {
        t.add_row(vec![
            p.code.clone(),
            crate::views::money_of(p.price.as_ref()),
            crate::views::money_of(p.regular_price.as_ref()),
            match (p.net_content, &p.net_content_uom) {
                (Some(n), Some(uom)) => format!("{n} {uom}"),
                _ => "—".into(),
            },
        ]);
    }
    writeln!(out, "{t}")?;
    crate::views::write_count(&mut out, products.len(), "product", None)?;
    // Named rather than counted: the batch endpoint drops a code it does not
    // recognise instead of failing, so a short answer is the only evidence.
    if products.len() < codes.len() {
        let found: Vec<&str> = products.iter().map(|p| p.code.as_str()).collect();
        let missing: Vec<&str> = codes
            .iter()
            .map(String::as_str)
            .filter(|c| !found.contains(c))
            .collect();
        writeln!(out, "No such product: {}.", missing.join(", "))?;
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct Prices<'a> {
    products: &'a [mitre10_api::ProductDetail],
}

impl cli_kit::View for Prices<'_> {
    fn text(&self, _out: &mut cli_kit::Out) -> std::io::Result<()> {
        Ok(())
    }
}
