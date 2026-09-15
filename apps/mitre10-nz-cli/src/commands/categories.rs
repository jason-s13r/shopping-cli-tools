//! `categories` -- the tree, out of the CMS navigation component.

use cli_kit::emit;

use crate::app::App;
use crate::error::AppResult;
use crate::views::CategoryTree;

pub async fn run(app: &App, query: Option<String>, depth: usize) -> AppResult<()> {
    let tree = app.client()?.category_tree().await?;
    let mut out = app.out();
    emit(
        &mut out,
        &CategoryTree {
            tree: &tree,
            query,
            depth,
        },
    )?;
    Ok(())
}
