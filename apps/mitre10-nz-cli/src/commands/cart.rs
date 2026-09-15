//! `cart` -- the basket, which needs no account.
//!
//! The cart id is kept in the state directory rather than the config file: it
//! is a handle the storefront issued, not a setting anyone chose, and a stale
//! one should cost a new basket rather than an edit.

use cli_kit::emit;
use mitre10_api::Fulfilment;

use crate::app::App;
use crate::cli::CartAction;
use crate::error::{AppError, AppResult};
use crate::views::CartView;

/// Where the current basket's id is filed.
const CART_FILE: &str = "cart";

pub async fn run(app: &App, action: CartAction) -> AppResult<()> {
    let client = app.client()?;

    match action {
        CartAction::New => {
            let cart = client.create_cart().await?;
            remember(app, &client, &cart)?;
            let mut out = app.out();
            emit(&mut out, &CartView::new(&cart))?;
        }
        CartAction::List => {
            let id = current(app)?;
            let cart = client.cart(&id).await?;
            let mut out = app.out();
            emit(&mut out, &CartView::new(&cart))?;
        }
        CartAction::Add {
            code,
            quantity,
            store,
            deliver,
        } => {
            let code = super::product_code(&code);
            // The storefront wants a store on every line, delivery or not: it
            // is where the stock is drawn from.
            let store = app.store(store).ok_or_else(|| {
                AppError::usage(
                    "no store to draw stock from; pass --store or set one with `mitre10 store set`",
                )
            })?;
            let id = match existing(app)? {
                Some(id) => id,
                None => {
                    let cart = client.create_cart().await?;
                    remember(app, &client, &cart)?;
                    cart_id(&client, &cart)?
                }
            };
            let cart = client
                .cart_add(&id, &code, quantity, &store, !deliver)
                .await?;
            let mut out = app.out();
            emit(&mut out, &CartView::changed(&cart))?;
        }
        CartAction::Set { item, quantity } => {
            let cart = client.cart_update(&current(app)?, item, quantity).await?;
            let mut out = app.out();
            emit(&mut out, &CartView::changed(&cart))?;
        }
        CartAction::Remove { item } => {
            let cart = client.cart_remove(&current(app)?, item).await?;
            let mut out = app.out();
            emit(&mut out, &CartView::changed(&cart))?;
        }
        CartAction::Fulfilment { how } => {
            let how = Fulfilment::parse(&how).ok_or_else(|| {
                AppError::usage(format!(
                    "{how:?} is not a way to get an order; use collect, delivery or express"
                ))
            })?;
            let cart = client.cart_fulfilment(&current(app)?, how).await?;
            let mut out = app.out();
            emit(&mut out, &CartView::changed(&cart))?;
        }
        CartAction::Voucher { code } => {
            let cart = client.cart_voucher(&current(app)?, &code).await?;
            let mut out = app.out();
            emit(&mut out, &CartView::changed(&cart))?;
        }
    }
    Ok(())
}

fn file(app: &App) -> std::path::PathBuf {
    app.paths.state_file(CART_FILE)
}

fn existing(app: &App) -> AppResult<Option<String>> {
    let path = file(app);
    if !path.exists() {
        return Ok(None);
    }
    let id = std::fs::read_to_string(&path)?.trim().to_string();
    Ok((!id.is_empty()).then_some(id))
}

fn current(app: &App) -> AppResult<String> {
    existing(app)?.ok_or_else(|| {
        AppError::usage(
            "no basket yet; `mitre10 cart add <code>` starts one, or `mitre10 cart new`",
        )
    })
}

fn remember(app: &App, client: &mitre10_api::Client, cart: &mitre10_api::Cart) -> AppResult<()> {
    let id = cart_id(client, cart)?;
    let path = file(app);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &id)?;
    net_kit::restrict(&path);
    Ok(())
}

fn cart_id(client: &mitre10_api::Client, cart: &mitre10_api::Cart) -> AppResult<String> {
    client.cart_id(cart).ok_or_else(|| {
        AppError::from(mitre10_api::Error::Shape(
            "the new cart carried no id".into(),
        ))
    })
}
