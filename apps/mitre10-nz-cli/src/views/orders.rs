//! What has been bought.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use mitre10_api::OrderPage;
use serde::Serialize;

#[derive(Serialize)]
pub struct OrderList<'a> {
    pub page: &'a OrderPage,
}

impl View for OrderList<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let p = self.page;
        if p.orders.is_empty() {
            return writeln!(out, "No orders.");
        }
        let mut t = table(&["Order", "Placed", "Status", "Items", "Total"]);
        for order in &p.orders {
            t.add_row(vec![
                order.code.clone(),
                super::or_dash(order.placed.as_deref()),
                super::or_dash(order.status.as_deref()),
                order.lines.len().to_string(),
                super::money_of(order.total.as_ref()),
            ]);
        }
        writeln!(out, "{t}")?;
        let more = p.page + 1 < p.pages;
        super::write_count(
            out,
            p.orders.len(),
            "order",
            more.then(|| format!("Next: --page {}", p.page + 1))
                .as_deref(),
        )
    }
}
