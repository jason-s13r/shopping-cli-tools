//! One product page.

use std::io::{self, Write};

use cli_kit::{section, table, Out, View};
use mitre10_api::ProductDetail;
use serde::Serialize;

#[derive(Serialize)]
pub struct ProductView<'a> {
    pub product: &'a ProductDetail,
}

impl View for ProductView<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let p = self.product;
        writeln!(
            out,
            "{}",
            out.heading(p.title.as_deref().unwrap_or(&p.name))
        )?;

        // A headless two-column table for the facts, the same as the other
        // tools' product pages, so a person reading two of them side by side
        // is reading one layout.
        let mut t = table(&["", ""]);
        t.add_row(vec!["Code".to_string(), p.code.clone()]);
        if let Some(brand) = &p.brand {
            t.add_row(vec!["Brand".to_string(), brand.clone()]);
        }
        t.add_row(vec![
            "Price".to_string(),
            super::price_label(
                p.price.as_ref().map(|m| m.value),
                p.regular_price.as_ref().map(|m| m.value),
            ),
        ]);
        if let Some(badge) = &p.price_badge {
            t.add_row(vec!["Offer".to_string(), badge.clone()]);
        }
        if let (Some(content), Some(uom)) = (p.net_content, &p.net_content_uom) {
            t.add_row(vec!["Size".to_string(), format!("{content} {uom}")]);
        }
        if let Some(model) = &p.model_number {
            t.add_row(vec!["Model".to_string(), model.clone()]);
        }
        if let Some(rating) = p.rating {
            // The count is routinely 0 while the rating is real: OCC carries
            // the average, and the reviews themselves live in Bazaarvoice.
            let from = match p.reviews {
                Some(n) if n > 0 => format!(" from {n} reviews"),
                _ => String::new(),
            };
            t.add_row(vec!["Rating".to_string(), format!("{rating:.1}{from}")]);
        }
        t.add_row(vec![
            "Available".to_string(),
            format!(
                "{}{}",
                if p.click_and_collect { "collect" } else { "" },
                if p.home_delivery {
                    if p.click_and_collect {
                        ", delivery"
                    } else {
                        "delivery"
                    }
                } else if p.click_and_collect {
                    ""
                } else {
                    "neither"
                }
            ),
        ]);
        if let Some(store) = &p.store_name {
            let level = p
                .stock_indicator
                .map(|s| s.label().to_string())
                .unwrap_or_else(|| "unknown".into());
            let count = p
                .store_stock
                .map(|n| format!(" ({n} on hand)"))
                .unwrap_or_default();
            t.add_row(vec!["At".to_string(), format!("{store}: {level}{count}")]);
        }
        if !p.breadcrumbs.is_empty() {
            t.add_row(vec!["In".to_string(), p.breadcrumbs.join(" > ")]);
        }
        writeln!(out, "{t}")?;

        if let Some(summary) = &p.summary {
            writeln!(out)?;
            writeln!(out, "{summary}")?;
        }

        if !p.specifications.is_empty() {
            section(out, "Specifications")?;
            let mut t = table(&["", ""]);
            for (key, value) in &p.specifications {
                t.add_row(vec![key.clone(), value.clone()]);
            }
            writeln!(out, "{t}")?;
        }

        if !p.variants.is_empty() {
            section(out, "Variants")?;
            let mut t = table(&["Code", "Name", "Price"]);
            for v in &p.variants {
                t.add_row(vec![
                    v.code.clone(),
                    super::or_dash(v.name.as_deref()),
                    super::money_of(v.price.as_ref()),
                ]);
            }
            writeln!(out, "{t}")?;
        }
        Ok(())
    }
}
