//! Showing the storefront to a person.
//!
//! Every type here is a [`cli_kit::View`] over a [`farmers_api`] type, which is
//! the whole shape of this layer: the protocol knows nothing about rendering,
//! the rendering knows nothing about HTTP, and `--json` falls out of the same
//! struct the text renderer reads rather than being written twice.
//!
//! These live in the app rather than in a library because there is only one
//! consumer. When a second department-store retailer wants them, they move to
//! `packages/` -- and not before.

mod account;
mod cart;
mod categories;
mod product;
mod products;
mod regions;
mod stock;

pub use account::AuthStatus;
pub use cart::{CartView, OrderList, WishlistView};
pub use categories::CategoryTree;
pub use product::{ProductView, VariantList};
pub use products::{ListingView, PriceList, Suggestions};
pub use regions::RegionList;
pub use stock::StockList;

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

/// The price, with the crossed-out one alongside when something is reduced.
///
/// The saving is worked out rather than read off: this API sends the ticket
/// price and the selling price as the same number for everything that is not
/// on special, so a pair printed unconditionally would claim a saving of zero
/// on the whole catalogue.
pub(crate) fn price_label(product: &farmers_api::Product) -> String {
    match (product.price(), product.was()) {
        (Some(now), Some(was)) => format!(
            "{} (was {}, save {})",
            now.display(),
            was.display(),
            product
                .saving()
                .map(|s| s.display())
                .unwrap_or_else(|| "—".into())
        ),
        (Some(now), None) => now.display(),
        (None, _) => "—".into(),
    }
}

/// A master's price range, for the one case where a single figure would lie.
pub(crate) fn range_label(product: &farmers_api::Product) -> Option<String> {
    let (min, max) = (product.min_price.as_ref()?, product.max_price.as_ref()?);
    (min.value < max.value).then(|| format!("{} – {}", min.display(), max.display()))
}

/// `yes` / `no`, plain.
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
    use farmers_api::{Money, Product};

    fn priced(list: f64, sale: f64) -> Product {
        Product {
            list_price: Some(Money::new(list, "NZD")),
            sale_price: Some(Money::new(sale, "NZD")),
            ..Default::default()
        }
    }

    #[test]
    fn a_saving_is_only_claimed_when_the_price_actually_fell() {
        assert_eq!(price_label(&priced(89.99, 89.99)), "$89.99");
        assert_eq!(
            price_label(&priced(89.99, 69.99)),
            "$69.99 (was $89.99, save $20.00)"
        );
    }

    #[test]
    fn a_missing_price_is_a_dash_rather_than_a_zero() {
        // "$0.00" and "no price" are different facts, and a hidden price is
        // ordinary on this catalogue.
        assert_eq!(price_label(&Product::default()), "—");
    }

    #[test]
    fn a_range_is_shown_only_when_there_is_actually_a_range() {
        // A master whose variants are all one price has min == max, and
        // "$89.99 – $89.99" reads as a bug.
        let mut p = Product {
            min_price: Some(Money::new(89.99, "NZD")),
            max_price: Some(Money::new(89.99, "NZD")),
            ..Default::default()
        };
        assert_eq!(range_label(&p), None);
        p.max_price = Some(Money::new(129.99, "NZD"));
        assert_eq!(range_label(&p).as_deref(), Some("$89.99 – $129.99"));
    }
}
