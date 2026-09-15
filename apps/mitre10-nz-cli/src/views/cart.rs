//! The basket and the saved items.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use mitre10_api::{Cart, Wishlist};
use serde::Serialize;

#[derive(Serialize)]
pub struct CartView<'a> {
    pub cart: &'a Cart,
    /// What the storefront said about a change it did not make in full. Part
    /// of the view rather than a `println!` so `--json` carries it too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<&'a str>,
}

impl<'a> CartView<'a> {
    pub fn new(cart: &'a Cart) -> CartView<'a> {
        CartView { cart, notice: None }
    }

    pub fn changed(change: &'a mitre10_api::CartChange) -> CartView<'a> {
        CartView {
            cart: &change.cart,
            notice: change.notice.as_deref(),
        }
    }
}

impl View for CartView<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let c = self.cart;
        // Before the table: it explains why the table is not what was asked
        // for, and after it the reader has already drawn their conclusion.
        if let Some(notice) = self.notice {
            writeln!(out, "{}", out.warn(notice))?;
            writeln!(out)?;
        }
        if c.lines.is_empty() {
            return writeln!(out, "The basket is empty.");
        }
        // "Item" is Hybris's own line number, because that is what `cart set`
        // and `cart remove` take -- not the product code, and not the row's
        // position.
        let mut t = table(&["Item", "Code", "Product", "Qty", "Each", "Total"]);
        for line in &c.lines {
            t.add_row(vec![
                line.entry_number.to_string(),
                line.code.clone(),
                line.name.clone(),
                line.quantity.to_string(),
                super::money_of(line.unit_price.as_ref()),
                super::money_of(line.total.as_ref()),
            ]);
        }
        writeln!(out, "{t}")?;

        // Subtotal only when something moved it: with nothing applied and
        // nothing to pay for carriage it repeats the total, and two identical
        // numbers read as a mistake.
        let discount = c.discounts.as_ref().filter(|d| d.value > 0.0);
        let carriage = c
            .delivery
            .as_ref()
            .and_then(|d| d.cost.as_ref())
            .filter(|c| c.value > 0.0);
        if discount.is_some() || carriage.is_some() {
            if let Some(subtotal) = &c.subtotal {
                writeln!(out, "Subtotal {}", subtotal.display())?;
            }
        }
        if let Some(discounts) = discount {
            writeln!(out, "Discounts -{}", discounts.display())?;
        }
        if let Some(delivery) = &c.delivery {
            writeln!(
                out,
                "Fulfilment {}{}",
                delivery.name.as_deref().unwrap_or("—"),
                delivery
                    .cost
                    .as_ref()
                    .map(|c| format!(" ({})", c.display()))
                    .unwrap_or_default()
            )?;
        }
        if let Some(store) = &c.pickup_store {
            writeln!(out, "Collect from {store}")?;
        }
        if !c.vouchers.is_empty() {
            writeln!(out, "Vouchers {}", c.vouchers.join(", "))?;
        }
        if let Some(total) = &c.total {
            writeln!(
                out,
                "{}",
                out.heading(&format!("Total {}", total.display()))
            )?;
        }
        super::write_count(out, c.lines.len(), "line", None)
    }
}

#[derive(Serialize)]
pub struct WishlistView<'a> {
    pub wishlist: &'a Wishlist,
}

impl View for WishlistView<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.wishlist.items.is_empty() {
            return writeln!(out, "Nothing saved.");
        }
        let mut t = table(&["Code", "Product", "Price"]);
        for item in &self.wishlist.items {
            t.add_row(vec![
                item.code.clone(),
                item.name.clone(),
                super::money_of(item.price.as_ref()),
            ]);
        }
        writeln!(out, "{t}")?;
        super::write_count(out, self.wishlist.items.len(), "saved item", None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cli_kit::{emit, Format};
    use mitre10_api::{CartLine, Delivery, Money};

    fn money(value: f64) -> Money {
        Money::new(value, "NZD")
    }

    fn cart() -> Cart {
        Cart {
            total_items: 1,
            total: Some(money(23.18)),
            subtotal: Some(money(23.18)),
            lines: vec![CartLine {
                entry_number: 0,
                code: "269938".into(),
                name: "Storage Bin & Lid".into(),
                quantity: 1,
                unit_price: Some(money(23.18)),
                total: Some(money(23.18)),
                updateable: true,
                ..CartLine::default()
            }],
            ..Cart::default()
        }
    }

    fn render(view: &CartView) -> String {
        let mut out = Out::buffer(Format::Text);
        emit(&mut out, view).expect("writes");
        out.into_string()
    }

    #[test]
    fn a_line_is_addressed_by_the_number_cart_set_takes() {
        // The column says "Item" because `cart set <ITEM>` is what it feeds.
        let c = cart();
        let text = render(&CartView::new(&c));
        assert!(text.contains("Item"), "{text}");
        assert!(text.contains("269938"), "{text}");
        assert!(text.contains("1 line."), "{text}");
    }

    #[test]
    fn the_subtotal_is_left_out_when_nothing_moved_it() {
        // It would repeat the total, and two identical numbers read as a
        // mistake rather than as an untouched cart.
        let text = render(&CartView::new(&cart()));
        assert!(!text.contains("Subtotal"), "{text}");
        assert!(text.contains("Total $23.18"), "{text}");
    }

    #[test]
    fn a_discount_brings_the_subtotal_back_so_the_total_adds_up() {
        let mut c = cart();
        c.discounts = Some(money(11.59));
        c.total = Some(money(11.59));
        let text = render(&CartView::new(&c));
        assert!(text.contains("Subtotal $23.18"), "{text}");
        assert!(text.contains("Discounts -$11.59"), "{text}");
        assert!(text.contains("Total $11.59"), "{text}");
    }

    #[test]
    fn carriage_does_the_same_because_it_is_inside_the_total() {
        let mut c = cart();
        c.delivery = Some(Delivery {
            code: None,
            name: Some("Standard".into()),
            cost: Some(money(6.0)),
        });
        c.total = Some(money(29.18));
        let text = render(&CartView::new(&c));
        assert!(text.contains("Subtotal $23.18"), "{text}");
        assert!(text.contains("Fulfilment Standard ($6.00)"), "{text}");
    }

    #[test]
    fn a_capped_line_says_so_above_the_table_rather_than_after_it() {
        // After the table the reader has already drawn their conclusion about
        // the quantity.
        let c = cart();
        let view = CartView {
            cart: &c,
            notice: Some("only 1 in stock"),
        };
        let text = render(&view);
        let notice = text.find("only 1 in stock").expect("said");
        assert!(notice < text.find("269938").expect("a row"), "{text}");
    }

    #[test]
    fn an_empty_basket_says_so_and_prints_no_table() {
        let c = Cart::default();
        assert_eq!(render(&CartView::new(&c)), "The basket is empty.\n");
    }
}
