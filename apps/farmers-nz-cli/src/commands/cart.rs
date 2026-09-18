//! `cart` -- the basket.
//!
//! Anonymous. A basket hangs off the session cookie, so all of this works
//! signed out and what is in it survives signing in afterwards.
//!
//! Lines are addressed by the number `cart list` shows rather than by their
//! real ids, which are opaque 24-character strings nobody is going to type.
//! That costs a `cart list` before every change, which is one small request
//! and worth it.

use cli_kit::emit;

use crate::app::App;
use crate::cli::CartAction;
use crate::commands::product_code;
use crate::error::{AppError, AppResult};
use crate::views::CartView;

pub async fn run(app: &App, action: CartAction) -> AppResult<()> {
    match action {
        CartAction::List => list(app).await,
        CartAction::Add { code, quantity } => add(app, &code, quantity).await,
        CartAction::Set { item, quantity } => set(app, item, quantity).await,
        CartAction::Remove { item } => remove(app, item).await,
        CartAction::Promo { code } => promo(app, &code).await,
    }
}

async fn list(app: &App) -> AppResult<()> {
    let cart = app.client()?.cart().await?;
    emit(&mut app.out(), &CartView { cart })?;
    Ok(())
}

async fn add(app: &App, code: &str, quantity: i64) -> AppResult<()> {
    if quantity < 1 {
        return Err(AppError::usage("a quantity has to be at least 1"));
    }
    let sku = product_code(code);
    let client = app.client()?;

    // Looked up before adding, for two reasons worth one request. A master
    // cannot be bought and the storefront answers an add for one by doing
    // nothing at all -- so this refuses it here, with the variants to pick
    // from. And a variant's own axes are what the add form sends.
    let product = client.product(&sku).await?;
    if product.master {
        let variants = client.variants(&sku).await?;
        let pick = variants
            .iter()
            .take(6)
            .map(|v| format!("  {}  {}", v.sku, v.label()))
            .collect::<Vec<_>>()
            .join("\n");
        let more = variants.len().saturating_sub(6);
        return Err(AppError::usage(format!(
            "{sku} is a master and cannot be bought; pick one of its variants:\n{pick}{}",
            match more {
                0 => String::new(),
                n => format!("\n  … and {n} more, from `farmers variants {sku}`"),
            }
        )));
    }

    let options: Vec<(String, String)> = product
        .variation_values
        .iter()
        // The axis id the form takes, which is the display name plus the
        // suffix the catalogue gives it.
        .map(|v| (format!("{}-DisplayName", v.axis), v.value.clone()))
        .collect();

    let cart = client.cart_add(&sku, quantity, &options).await?;
    println!(
        "Added {} × {}.",
        quantity,
        product.name.as_deref().unwrap_or(&sku)
    );
    emit(&mut app.out(), &CartView { cart })?;
    Ok(())
}

async fn set(app: &App, item: usize, quantity: i64) -> AppResult<()> {
    if quantity < 0 {
        return Err(AppError::usage("a quantity cannot be negative"));
    }
    let client = app.client()?;
    let cart = client.cart().await?;
    let line = nth(&cart, item)?;
    let cart = client
        .cart_update(&[(line.id.clone(), quantity)], None)
        .await?;
    emit(&mut app.out(), &CartView { cart })?;
    Ok(())
}

async fn remove(app: &App, item: usize) -> AppResult<()> {
    let client = app.client()?;
    let cart = client.cart().await?;
    let line = nth(&cart, item)?;
    let name = line.name.clone();
    let cart = client.cart_remove(&line.id).await?;
    println!("Removed {}.", name.as_deref().unwrap_or("the line"));
    emit(&mut app.out(), &CartView { cart })?;
    Ok(())
}

async fn promo(app: &App, code: &str) -> AppResult<()> {
    let client = app.client()?;
    let cart = client.cart().await?;
    if cart.is_empty() {
        return Err(AppError::usage(
            "a promotion code needs something in the basket to apply to",
        ));
    }
    // Every line's current quantity travels with it, because the form posts
    // the whole basket rather than one field.
    let quantities: Vec<(String, i64)> = cart
        .lines
        .iter()
        .map(|l| (l.id.clone(), l.quantity.unwrap_or(1)))
        .collect();
    let after = client.cart_update(&quantities, Some(code.trim())).await?;

    // The storefront does not say "that code is no good" in any machine
    // readable way, so this compares the totals and reports what it can
    // actually see rather than claiming a discount was applied.
    let before = cart.total.as_ref().map(|m| m.value);
    let now = after.total.as_ref().map(|m| m.value);
    match (before, now) {
        (Some(before), Some(now)) if now < before => {
            println!("Applied {code}, saving ${:.2}.", before - now)
        }
        _ => println!("{code} did not change the total; it may not apply to this basket."),
    }
    emit(&mut app.out(), &CartView { cart: after })?;
    Ok(())
}

/// A line by the number a listing showed, or a usage failure naming the range.
fn nth(cart: &farmers_api::Cart, item: usize) -> AppResult<&farmers_api::CartLine> {
    cart.nth(item).ok_or_else(|| {
        AppError::usage(match cart.lines.len() {
            0 => "the basket is empty".to_string(),
            n => format!("there is no line {item}; `farmers cart list` shows 1 to {n}"),
        })
    })
}
