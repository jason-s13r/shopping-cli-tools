//! Showing the storefront to a person.
//!
//! Every type here is a [`cli_kit::View`] over an [`mitre10_api`] type, which is
//! the whole shape of this layer: the protocol knows nothing about rendering,
//! the rendering knows nothing about HTTP, and `--json` falls out of the same
//! struct the text renderer reads rather than being written twice.
//!
//! These live in the app rather than in a library because there is only one
//! consumer. When a second hardware retailer wants them, they move to
//! `packages/` -- and not before.

mod account;
mod cart;
mod categories;
mod orders;
mod product;
mod products;
mod stock;
mod stores;

pub use account::AuthStatus;
pub use cart::{CartView, WishlistView};
pub use categories::CategoryTree;
pub use orders::OrderList;
pub use product::ProductView;
pub use products::{ProductList, Suggestions};
pub use stock::StockList;
pub use stores::{StoreList, StoreView};

use cli_kit::{plural, Out};
use std::io::Write;

/// `3 stores. Select one: <what the caller said to run>`.
///
/// Shared by every listing so the shape is the same, and so the command half
/// is the caller's to supply -- these types do not know what the binary is
/// called.
pub(crate) fn write_count(
    out: &mut Out,
    count: usize,
    noun: &str,
    next: Option<&str>,
) -> std::io::Result<()> {
    let noun = format!("{noun}{}", plural(count));
    write_counted(out, count, &noun, next)
}

/// The same, for a noun [`cli_kit::plural`] cannot make: it appends an `s`,
/// and "categorys" is not a word.
pub(crate) fn write_irregular_count(
    out: &mut Out,
    count: usize,
    singular: &str,
    plural_form: &str,
    next: Option<&str>,
) -> std::io::Result<()> {
    let noun = if count == 1 { singular } else { plural_form };
    write_counted(out, count, noun, next)
}

fn write_counted(
    out: &mut Out,
    count: usize,
    noun: &str,
    next: Option<&str>,
) -> std::io::Result<()> {
    match next {
        Some(next) => writeln!(out, "{count} {noun}. {next}"),
        None => writeln!(out, "{count} {noun}."),
    }
}

pub fn money_of(m: Option<&mitre10_api::Money>) -> String {
    m.map_or_else(|| "—".into(), |m| m.display())
}

/// The price, with the crossed-out one alongside when something is reduced.
pub(crate) fn price_label(price: Option<f64>, was: Option<f64>) -> String {
    match (price, was) {
        // Only claim a saving when the maths is right: the index sends the
        // ticket price and the selling price as the same number for everything
        // that is not on special.
        (Some(now), Some(was)) if was > now => {
            format!("${now:.2} (was ${was:.2}, save ${:.2})", was - now)
        }
        (Some(now), _) => format!("${now:.2}"),
        (None, _) => "—".into(),
    }
}

/// `yes` / `no` / `—`, plain.
///
/// Plain on purpose: a coloured cell is measured by its bytes and breaks
/// `comfy-table`'s column rules.
pub(crate) fn yes_no(value: bool) -> String {
    if value {
        "yes".into()
    } else {
        "no".into()
    }
}

pub(crate) fn or_dash(value: Option<&str>) -> String {
    value.unwrap_or("—").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saving_is_only_claimed_when_the_price_actually_fell() {
        assert_eq!(price_label(Some(79.0), Some(79.0)), "$79.00");
        assert_eq!(
            price_label(Some(69.0), Some(79.0)),
            "$69.00 (was $79.00, save $10.00)"
        );
        assert_eq!(price_label(None, Some(79.0)), "—");
    }

    #[test]
    fn a_missing_price_is_a_dash_rather_than_a_zero() {
        // "$0.00" and "no price" are different facts, and a hidden price is
        // ordinary on this catalogue.
        assert_eq!(money_of(None), "—");
        assert_eq!(
            money_of(Some(&mitre10_api::Money::new(0.0, "NZD"))),
            "$0.00"
        );
    }
}
