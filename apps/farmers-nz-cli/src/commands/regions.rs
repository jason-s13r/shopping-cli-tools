//! `regions` -- what `stock --region` accepts.

use cli_kit::emit;

use crate::app::App;
use crate::error::AppResult;
use crate::views::RegionList;

pub fn run(app: &App) -> AppResult<()> {
    let mut out = app.out();
    // No request: the list is New Zealand's regions and does not change.
    emit(&mut out, &RegionList::all(app.region(None)))?;
    Ok(())
}
