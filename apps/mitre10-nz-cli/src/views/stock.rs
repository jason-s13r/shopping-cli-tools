//! Which stores have a product.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use mitre10_api::Stock;
use serde::Serialize;

#[derive(Serialize)]
pub struct StockList<'a> {
    pub code: &'a str,
    pub stock: &'a [Stock],
}

impl View for StockList<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if self.stock.is_empty() {
            return writeln!(out, "No store reports stock of {}.", self.code);
        }
        let mut t = table(&["ID", "Store", "Stock"]);
        for s in self.stock {
            t.add_row(vec![
                s.store.clone(),
                s.store_name.clone(),
                s.level.label().to_string(),
            ]);
        }
        writeln!(out, "{t}")?;
        // Written out rather than passed to `write_count`, which puts a full
        // stop after the count: "3 stores. of 3 listed have it" is not a
        // sentence.
        let available = self.stock.iter().filter(|s| s.level.is_available()).count();
        let listed = self.stock.len();
        writeln!(
            out,
            "{available} of {listed} store{} listed {} it.",
            cli_kit::plural(listed),
            if available == 1 { "has" } else { "have" }
        )
    }
}
