//! `doctor` -- what is set up, and whether it works.
//!
//! The same two halves as every other tool here. The header is what this
//! program decided before talking to anyone: where its files are, which store
//! is the default. Then one section for the retailer, ending in live calls,
//! because "configured" and "working" are different claims and only the second
//! is worth much.
//!
//! Two live calls rather than one, because there are two independent ways for
//! this to be broken. The search index is Algolia and needs no CORS grace; the
//! storefront is the one that enforces the `Origin`. Reporting them together
//! would make a wrong origin look like an outage.

use std::io::Write;
use std::time::Duration;

use cli_kit::{emit, field, human_duration, indented, section, verdict, Out, View};
use mitre10_api::{Query, StoredSession};
use net_kit::jwt;
use serde::Serialize;

use crate::app::App;
use crate::error::{AppError, AppResult};

pub async fn run(app: &App) -> AppResult<()> {
    let shop = examine(app).await;
    let report = Doctor {
        // Named, because a report gets pasted into a bug and "0.1.0" alone
        // does not say what of.
        version: format!("mitre10 {}", crate::build::short_version()),
        config_file: format!(
            "{} ({})",
            app.config_file.display(),
            if app.config_file.exists() {
                "present"
            } else {
                "not written yet"
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
    if healthy {
        Ok(())
    } else {
        Err(AppError::Reported(1))
    }
}

async fn examine(app: &App) -> Shop {
    let endpoints = app.endpoints();
    let client = match app.client() {
        Ok(client) => client,
        Err(e) => {
            return Shop {
                storefront: endpoints.site,
                search: Err(e.to_string()),
                store: None,
                login: Login::Error(e.to_string()),
                reachable: Err(e.to_string()),
                healthy: false,
            }
        }
    };

    // The index first: it needs no credentials and no CORS grace, so a failure
    // here is the network rather than anything this tool decided.
    let search = match client
        .listing(&Query::search("paint").with_page_size(Some(1)))
        .await
    {
        Ok(listing) => Ok(format!("{} products for \"paint\"", listing.total)),
        Err(e) => Err(e.to_string()),
    };

    // Then the storefront, which is the one that enforces the CORS headers --
    // so this is what catches a wrong Origin.
    let reachable = match client.stock("174969").await {
        Ok(stock) => Ok(format!(
            "{} stores answered for a known product",
            stock.len()
        )),
        Err(e) if matches!(e, mitre10_api::Error::Cors) => {
            Err(format!("{e}; check M10_SITE_ORIGIN"))
        }
        Err(e) => Err(e.to_string()),
    };

    let store = match app.store(None) {
        None => None,
        Some(code) => Some(match client.store(&code).await {
            Ok(store) => Ok(format!("{code} ({})", store.name)),
            Err(e) => Err(format!("{code}, {e}")),
        }),
    };

    // Credentials last, and never a fault: everything above works signed out,
    // and most of this tool does too.
    let session = StoredSession::load(&app.secrets())
        .ok()
        .flatten()
        .map(|s| s.session())
        .unwrap_or_default();
    let login = if !session.can_refresh() {
        Login::SignedOut
    } else {
        // Spend the credential rather than reporting that one exists: a stored
        // sign-in the storefront has since dropped looks identical from here
        // until it is used.
        match client.customer().await {
            Ok(customer) => Login::In {
                account: customer
                    .email
                    .or(session.email.clone())
                    .unwrap_or_else(|| "signed in".into()),
                expires_in: session
                    .expires_at
                    .and_then(|at| at.checked_sub(jwt::now_ms()))
                    .map(|left| left / 1000),
            },
            Err(e) => Login::Error(e.to_string()),
        }
    };

    Shop {
        storefront: endpoints.site,
        // A store nobody has selected and nobody has signed in to are both
        // ordinary states, so neither decides health. Reaching the site does.
        healthy: search.is_ok() && reachable.is_ok() && !matches!(login, Login::Error(_)),
        search,
        store,
        login,
        reachable,
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
    search: Result<String, String>,
    /// `None` when no store is selected, which is not a failure to report.
    #[serde(skip_serializing_if = "Option::is_none")]
    store: Option<Result<String, String>>,
    login: Login,
    reachable: Result<String, String>,
    healthy: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Login {
    SignedOut,
    In {
        account: String,
        expires_in: Option<u64>,
    },
    Error(String),
}

impl View for Doctor {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        writeln!(out, "{}", self.version)?;
        field(out, "config file", &self.config_file)?;
        field(out, "state dir", &self.state_dir)?;
        field(out, "secrets", &self.secrets)?;

        section(out, "Mitre 10")?;
        indented(out, "storefront", &self.shop.storefront)?;
        match &self.shop.search {
            Ok(detail) => indented(out, "search", &format!("{}, {detail}", out.good("ok")))?,
            Err(e) => indented(out, "search", &format!("{}, {e}", out.bad("no")))?,
        }
        match &self.shop.store {
            Some(Ok(store)) => indented(out, "store", store)?,
            Some(Err(e)) => indented(out, "store", &format!("{}, {e}", out.bad("no")))?,
            // Not a warning: only `cart` needs one, and it says so itself.
            None => indented(out, "store", &out.dim("none selected"))?,
        }
        indented(out, "login", &describe(out, &self.shop.login))?;
        // Not "storefront": that label is already a hostname above, and the
        // same word for a setting and for a result reads as a contradiction
        // when one says a URL and the other says "yes".
        match &self.shop.reachable {
            Ok(detail) => indented(out, "reachable", &format!("{}, {detail}", out.good("yes")))?,
            Err(e) => indented(out, "reachable", &format!("{}, {e}", out.bad("no")))?,
        }

        verdict(out, self.shop.healthy)
    }
}

fn describe(out: &Out, login: &Login) -> String {
    match login {
        Login::SignedOut => out.dim("signed out").to_string(),
        Login::Error(e) => format!("{}, {e}", out.bad("refused")),
        Login::In {
            account,
            expires_in,
        } => match expires_in {
            Some(secs) => format!(
                "{account}, token expires in {}",
                human_duration(Duration::from_secs(*secs))
            ),
            // Not a fault: the next command mints one from the stored refresh
            // token without asking anybody anything.
            None => format!("{account}, {}", out.dim("token will be minted")),
        },
    }
}
