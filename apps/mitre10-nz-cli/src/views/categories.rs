//! The category tree.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use mitre10_api::CategoryNode;
use serde::Serialize;

#[derive(Serialize)]
pub struct CategoryTree<'a> {
    pub tree: &'a CategoryNode,
    #[serde(skip)]
    pub query: Option<String>,
    #[serde(skip)]
    pub depth: usize,
}

impl View for CategoryTree<'_> {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let query = self.query.as_ref().map(|q| q.to_lowercase());
        let mut rows = Vec::new();
        self.tree.walk(&mut |node, path| {
            if node.code.is_empty() || path.len() > self.depth {
                return;
            }
            let trail = path.join(" > ");
            if let Some(query) = &query {
                let haystack = format!("{} {trail}", node.name).to_lowercase();
                if !haystack.contains(query) {
                    return;
                }
            }
            rows.push((node.code.clone(), node.name.clone(), trail, node.level()));
        });

        if rows.is_empty() {
            return writeln!(out, "No categories matched.");
        }
        let mut t = table(&["Code", "Category", "Under", "Browsable"]);
        for (code, name, trail, level) in &rows {
            t.add_row(vec![
                code.clone(),
                name.clone(),
                trail.clone(),
                // A landing page cannot be browsed, and saying so here saves
                // someone running a browse that matches nothing.
                if level.is_some() { "yes" } else { "no" }.to_string(),
            ]);
        }
        writeln!(out, "{t}")?;
        super::write_irregular_count(
            out,
            rows.len(),
            "category",
            "categories",
            Some("Browse one with `mitre10 browse <code>`."),
        )
    }
}
