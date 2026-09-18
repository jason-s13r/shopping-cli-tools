//! One product, and what can be bought under it.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use farmers_api::{Product, Variant};
use serde::Serialize;

use super::{price_label, range_label, write_count, yes_no};

#[derive(Serialize)]
pub struct ProductView {
    pub product: Product,
    /// Present when `--variants` asked for them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<Variant>>,
}

impl View for ProductView {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let p = &self.product;
        writeln!(out, "{}", out.heading(p.name.as_deref().unwrap_or(&p.sku)))?;

        let mut t = table(&["", ""]);
        t.add_row(vec!["Code", &p.sku]);
        if let Some(brand) = &p.brand {
            t.add_row(vec!["Brand", brand]);
        }
        // A master's single price is its cheapest variant's, which is
        // misleading on its own -- so the range wins where there is one.
        match range_label(p) {
            Some(range) => t.add_row(vec!["Price", &range]),
            None => t.add_row(vec!["Price", &price_label(p)]),
        };
        if !p.category_path.is_empty() {
            t.add_row(vec!["Category", &p.breadcrumb()]);
        }
        if p.master {
            // Said plainly, because the code someone pasted cannot be bought
            // and nothing else on this screen would tell them.
            t.add_row(vec!["Kind", "a master — pick a variant below to buy"]);
        } else {
            t.add_row(vec!["In stock", &yes_no(p.in_stock.unwrap_or(false))]);
            if let Some(stock) = p.available_stock {
                t.add_row(vec!["Available", &stock.to_string()]);
            }
        }
        if let Some(master) = &p.master_sku {
            t.add_row(vec!["Master", master]);
        }
        if let Some((min, max)) = p.ships_in {
            t.add_row(vec!["Dispatch", &format!("{min}–{max} working days")]);
        }
        if let Some(rating) = p.rating {
            t.add_row(vec![
                "Rating",
                &format!("{rating:.1} from {} reviews", p.review_count.unwrap_or(0)),
            ]);
        }
        if !p.variation_values.is_empty() {
            let values: Vec<String> = p
                .variation_values
                .iter()
                .map(|v| format!("{} {}", v.axis, v.value))
                .collect();
            t.add_row(vec!["Variation", &values.join(", ")]);
        }
        if !p.promotions.is_empty() {
            t.add_row(vec!["Promotions", &p.promotions.join("; ")]);
        }
        if let Some(image) = p.image() {
            t.add_row(vec!["Image", &image.url]);
        }
        writeln!(out, "{t}")?;

        if let Some(description) = &p.short_description {
            writeln!(out, "{description}")?;
        }
        if let Some(variants) = &self.variants {
            writeln!(out)?;
            VariantList {
                sku: p.sku.clone(),
                variants: variants.clone(),
            }
            .text(out)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct VariantList {
    pub sku: String,
    pub variants: Vec<Variant>,
}

impl View for VariantList {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.variants.is_empty() {
            return writeln!(out, "{} has no variants; it is a single product.", self.sku);
        }
        let mut t = table(&["Code", "Variant", "In stock", "Available"]);
        for v in &self.variants {
            t.add_row(vec![
                // The default is marked rather than sorted first: the site's
                // own order is size order, and re-sorting it would put 2XL
                // above S.
                match v.default {
                    true => format!("{} *", v.sku),
                    false => v.sku.clone(),
                },
                v.label(),
                yes_no(v.in_stock.unwrap_or(false)),
                v.available_stock
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".into()),
            ]);
        }
        writeln!(out, "{t}")?;
        write_count(
            out,
            self.variants.len(),
            "variant",
            Some("* is the default."),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cli_kit::{emit, Format};
    use farmers_api::{CategoryRef, Money, VariationValue};

    fn render<V: View>(view: &V) -> String {
        let mut out = Out::buffer(Format::Text);
        emit(&mut out, view).expect("writes");
        out.into_string()
    }

    fn variant() -> Product {
        Product {
            sku: "6867065002".into(),
            name: Some("Chisel Fleece Robe, Charcoal".into()),
            brand: Some("Chisel".into()),
            list_price: Some(Money::new(89.99, "NZD")),
            sale_price: Some(Money::new(69.99, "NZD")),
            in_stock: Some(true),
            available_stock: Some(31),
            master_sku: Some("6867065".into()),
            category_path: vec![CategoryRef {
                id: "51-03".into(),
                name: "Men".into(),
            }],
            variation_values: vec![VariationValue {
                axis: "Size".into(),
                value: "L-XL".into(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_product_leads_with_its_name_and_shows_the_saving() {
        let text = render(&ProductView {
            product: variant(),
            variants: None,
        });
        assert!(text.starts_with("Chisel Fleece Robe, Charcoal"), "{text}");
        assert!(text.contains("was $89.99, save $20.00"), "{text}");
        assert!(text.contains("Men"), "{text}");
    }

    #[test]
    fn a_master_says_it_cannot_be_bought_and_shows_a_range_not_a_figure() {
        // Someone pastes the code off a search result; nothing else on the
        // screen would tell them it is not the thing to buy, and its single
        // price is its cheapest variant's.
        let master = Product {
            master: true,
            min_price: Some(Money::new(89.99, "NZD")),
            max_price: Some(Money::new(129.99, "NZD")),
            ..variant()
        };
        let text = render(&ProductView {
            product: master,
            variants: None,
        });
        assert!(text.contains("pick a variant"), "{text}");
        assert!(text.contains("$89.99 – $129.99"), "{text}");
        assert!(!text.contains("In stock"), "a master has no stock: {text}");
    }

    #[test]
    fn variants_mark_the_default_rather_than_reordering_them() {
        // The site's order is size order; sorting the default first would put
        // 2XL above S.
        let text = render(&VariantList {
            sku: "6867065".into(),
            variants: vec![
                Variant {
                    sku: "6867065001".into(),
                    name: None,
                    values: vec![VariationValue {
                        axis: "Size".into(),
                        value: "S-M".into(),
                    }],
                    in_stock: Some(true),
                    available_stock: Some(14),
                    default: false,
                },
                Variant {
                    sku: "6867065002".into(),
                    name: None,
                    values: vec![VariationValue {
                        axis: "Size".into(),
                        value: "L-XL".into(),
                    }],
                    in_stock: Some(true),
                    available_stock: Some(31),
                    default: true,
                },
            ],
        });
        let first = text.find("6867065001").expect("first");
        let second = text.find("6867065002").expect("second");
        assert!(first < second, "order kept: {text}");
        assert!(text.contains("6867065002 *"), "{text}");
        assert!(text.contains("* is the default."), "{text}");
    }

    #[test]
    fn a_product_with_no_variants_says_so_rather_than_drawing_an_empty_table() {
        let text = render(&VariantList {
            sku: "1234567".into(),
            variants: Vec::new(),
        });
        assert!(text.contains("no variants"), "{text}");
    }
}
