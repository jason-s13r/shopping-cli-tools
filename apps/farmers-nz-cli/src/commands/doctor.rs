//! `doctor` -- what is set up, and whether it works.
//!
//! The same two halves as every other tool here. The header is what this
//! program decided before talking to anyone; then one section for the
//! retailer, ending in live calls, because "configured" and "working" are
//! different claims and only the second is worth much.
//!
//! Two live calls rather than one, and they are chosen to be independent. The
//! search index is a different company on a different host with no bot
//! manager; the storefront is the half that can be refused. Reporting them
//! together would make a bot-manager refusal look like an outage, which is the
//! single most confusing thing this storefront does.

use std::io::Write;

use cli_kit::{emit, field, indented, section, verdict, Out, View};
use farmers_api::{Query, StoredSession};
use serde::Serialize;

use crate::app::App;
use crate::error::{AppError, AppResult};

pub async fn run(app: &App) -> AppResult<()> {
    let shop = examine(app).await;
    let report = Doctor {
        // Named, because a report gets pasted into a bug and "0.1.0" alone
        // does not say what of.
        version: format!("farmers {}", crate::build::short_version()),
        config_file: format!(
            "{} ({})",
            app.config_file.display(),
            match app.config_file.exists() {
                true => "present",
                false => "not written yet",
            }
        ),
        state_dir: app.paths.state_dir.display().to_string(),
        secrets: app.secrets().backend().describe().to_string(),
        shop,
    };

    let healthy = report.shop.healthy;
    emit(&mut app.out(), &report)?;
    // The report already said what failed and why, so this only carries the
    // code -- `doctor` in a script should not need its output parsed.
    match healthy {
        true => Ok(()),
        false => Err(AppError::Reported(1)),
    }
}

async fn examine(app: &App) -> Shop {
    let endpoints = app.endpoints();
    let client = match app.client() {
        Ok(client) => client,
        Err(e) => {
            return Shop {
                storefront: endpoints.origin,
                browser: crate::browser::available(app.env.browser_python.as_deref()),
                search: Err(e.to_string()),
                region: None,
                warm: Err(e.to_string()),
                login: Login::SignedOut,
                healthy: false,
            }
        }
    };

    // The index first: a different host with no bot manager, so a failure here
    // is the network rather than anything Farmers decided about this client.
    let search = match client.search("robe", &Query::new().per_page(Some(1))).await {
        Ok(listing) => Ok(format!("{} products for \"robe\"", listing.total)),
        Err(e) => Err(e.to_string()),
    };

    // Then the storefront, which is the half that gets refused. One category
    // call: it warms first, so it exercises both halves of the gated path at
    // the cost of one request rather than two.
    let warm = match client.categories(1).await {
        Ok(categories) => Ok(format!("warmed, {} top-level categories", categories.len())),
        // Not a wait and not a fingerprint: admission is only granted to
        // cookies a browser earned, so this line names the browser.
        Err(e) if e.is_denied() => Err(format!(
            "{e}. Admission comes from a browser, so check the one above and try --headful"
        )),
        Err(e) => Err(e.to_string()),
    };

    let region = app
        .region(None)
        .map(|code| match farmers_api::region_name(&code) {
            Some(name) => Ok(format!("{code} ({name})")),
            None => Err(format!("{code} is not a region this tool knows")),
        });

    // Credentials last, and never a fault: everything above works signed out,
    // and so does most of this tool.
    let session = StoredSession::load(&app.secrets())
        .ok()
        .flatten()
        .map(|s| s.session())
        .unwrap_or_default();
    let login = if !session.account() {
        Login::SignedOut
    } else {
        // Spend the session rather than reporting that one exists: a stored
        // cookie the storefront has since dropped looks identical from here
        // until it is used.
        match client.whoami().await {
            Ok(account) if account.signed_in => Login::In {
                account: account
                    .name()
                    .or(account.email)
                    .or(session.email.clone())
                    .unwrap_or_else(|| "signed in".into()),
            },
            Ok(_) => Login::Stale,
            Err(e) => Login::Error(e.to_string()),
        }
    };

    Shop {
        storefront: endpoints.origin,
        browser: crate::browser::available(app.env.browser_python.as_deref()),
        // A region nobody has chosen and nobody being signed in are both
        // ordinary states, so neither decides health. Reaching the site does.
        // A stale login does not either: it is news, and the line says so.
        healthy: search.is_ok() && warm.is_ok(),
        search,
        region,
        warm,
        login,
    }
}

#[derive(Serialize)]
struct Doctor {
    version: String,
    config_file: String,
    state_dir: String,
    secrets: String,
    shop: Shop,
}

#[derive(Serialize)]
struct Shop {
    storefront: String,
    /// Whether there is a browser to buy admission with. Reported before the
    /// live calls because it is what decides whether the gated half can work
    /// at all, and a missing one explains every failure below it.
    browser: bool,
    search: Result<String, String>,
    /// `None` when no region is configured, which is not a failure to report.
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<Result<String, String>>,
    /// Whether the gated half of the storefront answered at all.
    warm: Result<String, String>,
    login: Login,
    healthy: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Login {
    SignedOut,
    In {
        account: String,
    },
    /// A cookie the storefront no longer recognises.
    Stale,
    Error(String),
}

impl View for Doctor {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        writeln!(out, "{}", self.version)?;
        field(out, "config file", &self.config_file)?;
        field(out, "state dir", &self.state_dir)?;
        field(out, "secrets", &self.secrets)?;

        section(out, "Farmers")?;
        indented(out, "storefront", &self.shop.storefront)?;
        indented(
            out,
            "browser",
            &match self.shop.browser {
                true => format!("{}, camoufox", out.good("ok")),
                // The sentence rather than a bare "no": this is the one line
                // in the report that someone can act on directly, and the
                // gated half of the tool does not work without it.
                false => format!("{}, {}", out.bad("none"), crate::browser::MISSING_BROWSER),
            },
        )?;
        match &self.shop.search {
            Ok(detail) => indented(out, "search", &format!("{}, {detail}", out.good("ok")))?,
            Err(e) => indented(out, "search", &format!("{}, {e}", out.bad("no")))?,
        }
        match &self.shop.warm {
            Ok(detail) => indented(out, "catalogue", &format!("{}, {detail}", out.good("ok")))?,
            Err(e) => indented(out, "catalogue", &format!("{}, {e}", out.bad("no")))?,
        }
        match &self.shop.region {
            Some(Ok(region)) => indented(out, "region", region)?,
            Some(Err(e)) => indented(out, "region", &format!("{}, {e}", out.bad("no")))?,
            // Not a warning: only `stock` uses one, and without it that
            // command asks every region instead.
            None => indented(out, "region", &out.dim("none selected"))?,
        }
        indented(out, "login", &describe(out, &self.shop.login))?;

        verdict(out, self.shop.healthy)
    }
}

fn describe(out: &Out, login: &Login) -> String {
    match login {
        Login::SignedOut => out.dim("signed out").to_string(),
        Login::In { account } => account.clone(),
        Login::Stale => out
            .warn("a stored session the storefront no longer knows; sign in again")
            .to_string(),
        Login::Error(e) => format!("{}, {e}", out.bad("unverified")),
    }
}
