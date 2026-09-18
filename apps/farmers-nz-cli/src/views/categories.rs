//! The category tree.

use std::io::{self, Write};

use cli_kit::{table, Out, View};
use farmers_api::Category;
use serde::Serialize;

use super::write_irregular_count;

#[derive(Serialize)]
pub struct CategoryTree {
    pub categories: Vec<Category>,
    /// What was searched for, when anything was.
    #[serde(skip)]
    pub query: Option<String>,
    #[serde(skip)]
    pub next: Option<String>,
}

impl View for CategoryTree {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        let rows = self.rows();
        if rows.is_empty() {
            return match &self.query {
                Some(q) => writeln!(out, "No category matches {q:?}."),
                None => writeln!(out, "No categories."),
            };
        }

        let mut t = table(&["Id", "Category", "Products"]);
        for (depth, category) in &rows {
            t.add_row(vec![
                category.id.clone(),
                // Indented rather than given a separate column: the tree is
                // the point, and a `Depth: 2` column says less than two spaces
                // do.
                format!("{}{}", "  ".repeat(*depth), category.name),
                category
                    .product_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".into()),
            ]);
        }
        writeln!(out, "{t}")?;
        write_irregular_count(
            out,
            rows.len(),
            "category",
            "categories",
            self.next.as_deref(),
        )
    }
}

impl CategoryTree {
    /// Depth and category, flattened, filtered by the query where there is
    /// one.
    ///
    /// A match keeps its ancestors' *depth* but not their rows: showing the
    /// path to every hit turns a three-word search into most of the tree.
    fn rows(&self) -> Vec<(usize, &Category)> {
        let wanted = self.query.as_ref().map(|q| q.to_lowercase());
        self.categories
            .iter()
            .flat_map(|root| root.flatten())
            .filter(|(above, category)| match &wanted {
                None => true,
                Some(q) => {
                    category.name.to_lowercase().contains(q)
                        || category.id.to_lowercase().contains(q)
                        || above.iter().any(|name| name.to_lowercase().contains(q))
                }
            })
            .map(|(above, category)| (above.len(), category))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cli_kit::{emit, Format};

    fn render(view: &CategoryTree) -> String {
        let mut out = Out::buffer(Format::Text);
        emit(&mut out, view).expect("writes");
        out.into_string()
    }

    fn tree() -> Vec<Category> {
        vec![Category {
            id: "51-03".into(),
            name: "Men".into(),
            product_count: Some(2481),
            children: vec![Category {
                id: "51-0303".into(),
                name: "Sleepwear, Robes & Slippers".into(),
                children: vec![Category {
                    id: "51-030302".into(),
                    name: "Robes".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }]
    }

    #[test]
    fn the_tree_is_shown_by_indentation_rather_than_by_a_depth_column() {
        let text = render(&CategoryTree {
            categories: tree(),
            query: None,
            next: None,
        });
        assert!(text.contains("  Sleepwear"), "{text}");
        assert!(text.contains("    Robes"), "{text}");
        assert!(text.contains("3 categories."), "{text}");
    }

    #[test]
    fn a_search_matches_an_ancestors_name_as_well_as_its_own() {
        // Someone looking for "men" wants what is under Men, and the leaves
        // are not called that.
        let text = render(&CategoryTree {
            categories: tree(),
            query: Some("men".into()),
            next: None,
        });
        assert!(text.contains("Robes"), "{text}");
    }

    #[test]
    fn a_search_returns_the_matches_and_not_the_whole_path_to_each() {
        let text = render(&CategoryTree {
            categories: tree(),
            query: Some("robes".into()),
            next: None,
        });
        // "Sleepwear, Robes & Slippers" and "Robes" both match; "Men" does not
        // and is not dragged in to give them somewhere to hang.
        assert!(!text.contains("| Men"), "{text}");
        assert!(text.contains("2 categories."), "{text}");
    }

    #[test]
    fn nothing_matching_says_what_was_searched_for() {
        let text = render(&CategoryTree {
            categories: tree(),
            query: Some("hammers".into()),
            next: None,
        });
        assert_eq!(text, "No category matches \"hammers\".\n");
    }
}
