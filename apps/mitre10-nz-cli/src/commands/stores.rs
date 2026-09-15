//! `stores` and `store` -- the stores, and which one commands use.

use cli_kit::emit;

use crate::app::App;
use crate::cli::StoreAction;
use crate::error::{AppError, AppResult};
use crate::views::{StoreList, StoreView};

pub async fn list(
    app: &App,
    query: Option<String>,
    postcode: Option<String>,
    near: Option<String>,
) -> AppResult<()> {
    let client = app.client()?;
    let mut stores = match (&postcode, &near) {
        (Some(_), Some(_)) => {
            return Err(AppError::usage(
                "--postcode and --near both say where to look; give one",
            ))
        }
        (Some(postcode), None) => client.stores_for_postcode(postcode).await?,
        (None, Some(point)) => {
            let (lat, lon) = parse_point(point)?;
            client.stores_near(lat, lon).await?
        }
        // No anchor: the storefront has no "every store" call, so a postcode
        // stands in. Auckland's central one reaches the widest list.
        (None, None) => {
            client
                .stores_for_postcode(app.postcode(None).as_deref().unwrap_or("1010"))
                .await?
        }
    };

    if let Some(query) = &query {
        let needle = query.to_lowercase();
        stores.retain(|s| {
            let town = s
                .address
                .as_ref()
                .and_then(|a| a.town.clone().or_else(|| a.suburb.clone()))
                .unwrap_or_default();
            format!("{} {town}", s.name)
                .to_lowercase()
                .contains(&needle)
        });
    }

    let mut out = app.out();
    emit(&mut out, &StoreList { stores: &stores })?;
    Ok(())
}

pub async fn store(app: &App, action: StoreAction) -> AppResult<()> {
    match action {
        StoreAction::Show => match app.store(None) {
            Some(code) => {
                let store = app.client()?.store(&code).await?;
                let mut out = app.out();
                emit(&mut out, &StoreView { store: &store })?;
            }
            None => println!("No store set. Select one: `mitre10 store set <id>`."),
        },
        StoreAction::Set { id } => {
            // Checked against the storefront before it is written: an id that
            // does not exist would otherwise fail on every later command with
            // no hint that this was where it came from.
            let store = app.client()?.store(&id).await?;
            let mut config = app.config.clone();
            config.set("store", &store.code)?;
            app.save(&config)?;
            println!("Using {} ({}).", store.name, store.code);
        }
        StoreAction::Clear => {
            let mut config = app.config.clone();
            config.unset("store")?;
            app.save(&config)?;
            println!("No store set.");
        }
    }
    Ok(())
}

/// `-36.85,174.76` -- the order the storefront takes them in.
fn parse_point(text: &str) -> AppResult<(f64, f64)> {
    let (lat, lon) = text
        .split_once(',')
        .ok_or_else(|| AppError::usage(format!("{text:?} is not a point; give it as `lat,lon`")))?;
    let lat: f64 = lat.trim().parse().map_err(|_| bad_point(text))?;
    let lon: f64 = lon.trim().parse().map_err(|_| bad_point(text))?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(bad_point(text));
    }
    Ok((lat, lon))
}

fn bad_point(text: &str) -> AppError {
    AppError::usage(format!(
        "{text:?} is not a point; give it as `lat,lon`, e.g. -36.85,174.76"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_is_latitude_then_longitude() {
        assert_eq!(
            parse_point("-36.85,174.76").expect("parses"),
            (-36.85, 174.76)
        );
        assert_eq!(
            parse_point(" -36.85 , 174.76 ").expect("parses"),
            (-36.85, 174.76)
        );
    }

    #[test]
    fn a_point_outside_the_world_is_refused() {
        // Swapping the two is the easy mistake, and New Zealand's longitude
        // is out of range as a latitude -- so it is catchable.
        assert!(parse_point("174.76,-36.85").is_err());
        assert!(parse_point("Auckland").is_err());
        assert!(parse_point("-36.85").is_err());
    }
}
