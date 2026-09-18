//! The basket, and the saved lists.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use farmers_api::{Cart, Order, Wishlist};
use serde::Serialize;

use super::{or_dash, write_count, write_irregular_count};

#[derive(Serialize)]
pub struct CartView {
    pub cart: Cart,
}

impl View for CartView {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.cart.is_empty() {
            return writeln!(out, "The basket is empty.");
        }

        let mut t = table(&["#", "Code", "Product", "Qty", "Each", "Total"]);
        for (i, line) in self.cart.lines.iter().enumerate() {
            let name = match line.label().is_empty() {
                true => or_dash(line.name.as_deref()),
                // The variant is part of what the line *is*; a basket with two
                // sizes of one robe is otherwise two identical rows.
                false => format!("{} ({})", or_dash(line.name.as_deref()), line.label()),
            };
            t.add_row(vec![
                // Counting from 1, because this is what `cart remove` takes --
                // the real ids are opaque and nobody is typing one.
                (i + 1).to_string(),
                or_dash(line.sku.as_deref()),
                name,
                line.quantity
                    .map(|q| q.to_string())
                    .unwrap_or_else(|| "—".into()),
                line.unit_price.as_ref().map_or("—".into(), |m| m.display()),
                line.total.as_ref().map_or("—".into(), |m| m.display()),
            ]);
        }
        writeln!(out, "{t}")?;

        let mut totals = table(&["", ""]);
        if let Some(subtotal) = &self.cart.subtotal {
            totals.add_row(vec!["Subtotal".to_string(), subtotal.display()]);
        }
        // Only when it differs: the storefront sends both for every basket, so
        // an unconditional row would claim a discount of nothing.
        if let (Some(discounted), Some(subtotal)) = (&self.cart.total, &self.cart.subtotal) {
            if discounted.value < subtotal.value {
                totals.add_row(vec!["After discounts".to_string(), discounted.display()]);
            }
        }
        if let Some(grand) = &self.cart.grand_total {
            totals.add_row(vec!["Total".to_string(), grand.display()]);
        }
        writeln!(out, "{totals}")?;

        // Delivery is not in this fragment at all -- the storefront works it
        // out at checkout -- so the total is not the final figure and saying
        // so beats implying otherwise.
        writeln!(
            out,
            "{}",
            out.dim("Delivery is worked out at checkout and is not included.")
        )?;
        write_count(
            out,
            self.cart.lines.len(),
            "line",
            Some(&format!("{} item(s).", self.cart.units())),
        )
    }
}

#[derive(Serialize)]
pub struct WishlistView {
    pub wishlists: Vec<Wishlist>,
}

impl View for WishlistView {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.wishlists.is_empty() {
            return writeln!(out, "No saved lists.");
        }
        for list in &self.wishlists {
            let title = match list.preferred {
                // Which list a bare `wishlist add` writes to, which is the one
                // thing worth knowing when there is more than one.
                true => format!("{} (preferred)", list.name),
                false => list.name.clone(),
            };
            writeln!(out, "{}", out.heading(&title))?;
            if list.items.is_empty() {
                writeln!(out, "  empty")?;
                continue;
            }
            let mut t = table(&["Code", "Product", "Price"]);
            for item in &list.items {
                t.add_row(vec![
                    or_dash(item.sku.as_deref()),
                    or_dash(item.name.as_deref()),
                    item.price.as_ref().map_or("—".into(), |m| m.display()),
                ]);
            }
            writeln!(out, "{t}")?;
        }
        write_count(out, self.wishlists.len(), "list", None)
    }
}

#[derive(Serialize)]
pub struct OrderList {
    pub orders: Vec<Order>,
}

impl View for OrderList {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.orders.is_empty() {
            return writeln!(out, "No orders.");
        }
        let mut t = table(&["Order", "Placed", "Status", "Total"]);
        for order in &self.orders {
            t.add_row(vec![
                order.number.clone(),
                or_dash(order.placed.as_deref()),
                or_dash(order.status.as_deref()),
                order.total.as_ref().map_or("—".into(), |m| m.display()),
            ]);
        }
        writeln!(out, "{t}")?;
        write_irregular_count(out, self.orders.len(), "order", "orders", None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cli_kit::{emit, Format};
    use farmers_api::{CartLine, Money, VariationValue};

    fn render<V: View>(view: &V) -> String {
        let mut out = Out::buffer(Format::Text);
        emit(&mut out, view).expect("writes");
        out.into_string()
    }

    fn line(id: &str, name: &str, qty: i64) -> CartLine {
        CartLine {
            id: id.into(),
            sku: Some("6867065002".into()),
            name: Some(name.into()),
            quantity: Some(qty),
            unit_price: Some(Money::new(89.99, "NZD")),
            total: Some(Money::new(89.99 * qty as f64, "NZD")),
            ..Default::default()
        }
    }

    #[test]
    fn a_basket_numbers_its_lines_because_the_real_ids_are_untypeable() {
        let text = render(&CartView {
            cart: Cart {
                lines: vec![line("notarealpli01", "A robe", 2)],
                subtotal: Some(Money::new(179.98, "NZD")),
                grand_total: Some(Money::new(179.98, "NZD")),
                ..Default::default()
            },
        });
        assert!(text.contains("1 line. 2 item(s)."), "{text}");
        assert!(!text.contains("notarealpli01"), "the id is noise: {text}");
    }

    #[test]
    fn two_sizes_of_one_product_are_told_apart_by_their_variant() {
        // Otherwise the basket shows two identical rows and neither `remove`
        // nor the eye can tell which is which.
        let mut small = line("a", "Chisel Fleece Robe", 1);
        small.options = vec![VariationValue {
            axis: "Size".into(),
            value: "S-M".into(),
        }];
        let mut large = line("b", "Chisel Fleece Robe", 1);
        large.options = vec![VariationValue {
            axis: "Size".into(),
            value: "L-XL".into(),
        }];

        let text = render(&CartView {
            cart: Cart {
                lines: vec![small, large],
                ..Default::default()
            },
        });
        assert!(text.contains("Size S-M"), "{text}");
        assert!(text.contains("Size L-XL"), "{text}");
    }

    #[test]
    fn a_discount_row_appears_only_when_something_was_actually_discounted() {
        // The storefront sends both figures for every basket.
        let same = render(&CartView {
            cart: Cart {
                lines: vec![line("a", "A robe", 1)],
                subtotal: Some(Money::new(89.99, "NZD")),
                total: Some(Money::new(89.99, "NZD")),
                ..Default::default()
            },
        });
        assert!(!same.contains("After discounts"), "{same}");

        let reduced = render(&CartView {
            cart: Cart {
                lines: vec![line("a", "A robe", 1)],
                subtotal: Some(Money::new(89.99, "NZD")),
                total: Some(Money::new(69.99, "NZD")),
                ..Default::default()
            },
        });
        assert!(reduced.contains("After discounts"), "{reduced}");
    }

    #[test]
    fn the_total_says_it_does_not_include_delivery() {
        // The fragment has no delivery figure in it, so the total is not the
        // final number and implying otherwise would be a lie about money.
        let text = render(&CartView {
            cart: Cart {
                lines: vec![line("a", "A robe", 1)],
                grand_total: Some(Money::new(89.99, "NZD")),
                ..Default::default()
            },
        });
        assert!(
            text.contains("Delivery is worked out at checkout"),
            "{text}"
        );
    }

    #[test]
    fn an_empty_basket_says_so_rather_than_drawing_an_empty_table() {
        let text = render(&CartView {
            cart: Cart::default(),
        });
        assert_eq!(text, "The basket is empty.\n");
    }

    #[test]
    fn the_preferred_list_is_marked_because_it_is_where_a_bare_add_goes() {
        let text = render(&WishlistView {
            wishlists: vec![
                Wishlist {
                    name: "Christmas".into(),
                    preferred: true,
                    ..Default::default()
                },
                Wishlist {
                    name: "Someday".into(),
                    ..Default::default()
                },
            ],
        });
        assert!(text.contains("Christmas (preferred)"), "{text}");
        assert!(text.contains("2 lists."), "{text}");
    }

    #[test]
    fn nothing_saved_and_nothing_bought_say_so_plainly() {
        assert_eq!(
            render(&WishlistView {
                wishlists: Vec::new()
            }),
            "No saved lists.\n"
        );
        assert_eq!(render(&OrderList { orders: Vec::new() }), "No orders.\n");
    }
}
