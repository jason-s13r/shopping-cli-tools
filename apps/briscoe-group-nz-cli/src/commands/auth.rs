//! `auth` -- getting the one credential that matters, and giving it up.
//!
//! There is really only one credential here: Gigya's `login_token`. The
//! storefront token everything else runs on is derived from it, lasts two
//! hours, and is reminted by any command that needs one -- nobody has to think
//! about it, and `status` reports it as "will renew" rather than as a problem.
//!
//! Getting that one credential is the hard part, and it is hard in exactly one
//! place: Gigya's `accounts.login` refuses a request with no reCAPTCHA token,
//! and refuses it *before* it looks at the password. So `login` drives a
//! browser -- headless by default -- whose entire job is to mint that token.
//! The email and password never enter it; this program makes the sign-in
//! request itself once the token is in hand.
//!
//! `token` is the way in that needs no browser at all, and it is unusually easy
//! here: the credential is a plain cookie on the site, `glt_<apiKey>`, so it
//! can be copied out of devtools' cookie list without touching the network tab.
//!
//! **Per fascia, always.** Briscoes and Rebel Sport are separate Gigya sites;
//! signing in to one does nothing for the other, and `status` shows both so
//! that is visible rather than surprising.

use bgnz_api::{auth, Banner, StoredSession};
use cli_kit::{emit, human_duration, prompt, prompt_password};
use net_kit::jwt;

use crate::app::App;
use crate::browser;
use crate::cli::AuthAction;
use crate::error::{AppError, AppResult};
use crate::views::{AuthStatus, BannerAuth};

pub async fn run(app: &App, action: AuthAction) -> AppResult<()> {
    match action {
        AuthAction::Login {
            email,
            password_command,
            headful,
        } => login(app, email, password_command, !headful).await,
        AuthAction::Token { token, cookie } => token_import(app, token, cookie).await,
        AuthAction::Refresh => refresh(app).await,
        AuthAction::Status => status(app),
        AuthAction::Logout { all } => logout(app, all),
    }
}

async fn login(
    app: &App,
    email: Option<String>,
    password_command: Option<String>,
    headless: bool,
) -> AppResult<()> {
    let client = app.client()?;
    let gigya = client.gigya_config().await?;

    let email = match email {
        Some(email) => email,
        None => prompt(&format!("{} email", app.banner))?,
    };
    let password = match &password_command {
        Some(command) => net_kit::run::capturing("password_command", command).await?,
        None => prompt_password("Password")?,
    };

    // The browser is given the origin and nothing else. It never sees the two
    // values above, which is the point of doing the sign-in here.
    eprintln!(
        "bgnz: minting a captcha token in a browser — the sign-in itself happens here, \
         and this is the only step that needs one."
    );
    let captcha = browser::captcha(
        app.env.browser_python.as_deref(),
        &app.paths.state_dir,
        &client.endpoints().origin,
        gigya.login_screen_set.as_deref(),
        gigya.login_start_screen.as_deref(),
        headless,
    )
    .await?;

    if app.env.debug {
        // The two things that decide whether a token is accepted, and the two
        // most likely to have moved when a sign-in starts failing.
        eprintln!(
            "bgnz: captcha site key {} action {}",
            captcha.site_key.as_deref().unwrap_or("?"),
            captcha.action.as_deref().unwrap_or("?")
        );
    }

    let login = auth::login(
        &app.http()?,
        client.endpoints(),
        app.banner,
        &gigya.api_key,
        &email,
        &password,
        &captcha.captcha_token,
    )
    .await
    .map_err(|e| {
        if auth::is_bad_credentials(&e) {
            AppError::usage(format!(
                "{} did not accept that email and password",
                app.banner
            ))
        } else {
            AppError::Api(e)
        }
    })?;

    let token = client.exchange(&login.assertion).await?;
    client.adopt(login, token);
    println!(
        "Signed in to {} as {email}. Nothing after this needs a browser.",
        app.banner
    );
    Ok(())
}

async fn token_import(app: &App, token: Option<String>, cookie: bool) -> AppResult<()> {
    let client = app.client()?;
    let gigya = client.gigya_config().await?;

    if cookie {
        println!("{}", browser::cookie_name(&gigya.api_key));
        println!(
            "Sign in to {} in a browser, then copy that cookie's value — it begins `st2.s.`.",
            client.endpoints().origin
        );
        return Ok(());
    }

    let token = match token {
        Some(token) => token,
        None => prompt_password(&format!(
            "{} login token (the {} cookie): ",
            app.banner,
            browser::cookie_name(&gigya.api_key)
        ))?,
    };
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::usage("no token given"));
    }

    // Spent immediately rather than filed on trust: a token that does not work
    // should fail here, while the person still has the browser open, not on
    // some later command that was about a price.
    let assertion = auth::assert(
        &app.http()?,
        client.endpoints(),
        app.banner,
        &gigya.api_key,
        &token,
        None,
    )
    .await?;
    let email = assertion.email.clone();
    let storefront_token = client.exchange(&assertion).await?;
    client.adopt(
        auth::Login {
            login_token: token,
            assertion,
        },
        storefront_token,
    );
    println!("Signed in to {} as {email}.", app.banner);
    Ok(())
}

async fn refresh(app: &App) -> AppResult<()> {
    // Built once: each client reads the credential store, which on a locked
    // keychain is a prompt.
    let client = app.client()?;
    client.renew().await?;
    let customer = client.customer().await?;
    println!(
        "{} token renewed for {}.",
        app.banner,
        customer.email.as_deref().unwrap_or("this account")
    );
    Ok(())
}

fn status(app: &App) -> AppResult<()> {
    // Both fascias, always. "Am I signed in" has two answers here, and showing
    // only the current one is how someone concludes the tool has forgotten
    // them.
    let banners = Banner::ALL
        .iter()
        .map(|&banner| {
            // Read the store directly rather than through a client, which
            // treats an unreadable store as an empty one. Here the
            // difference matters: a dismissed keychain prompt is not a signed
            // out account.
            let stored = StoredSession::load(&app.secrets_for(banner), banner);
            let unreadable = stored.as_ref().err().map(ToString::to_string);
            let session = stored
                .ok()
                .flatten()
                .map(|s| s.session())
                .unwrap_or_default();
            BannerAuth {
                banner: banner.name().to_string(),
                signed_in: session.can_refresh(),
                email: session.email.clone(),
                token_fresh: session.token_fresh(),
                expires_in: session.expires_at.and_then(|at| {
                    at.checked_sub(jwt::now_ms())
                        .map(|left| human_duration(std::time::Duration::from_millis(left)))
                }),
                unreadable,
            }
        })
        .collect();

    let mut out = app.out();
    emit(&mut out, &AuthStatus { banners })?;
    Ok(())
}

fn logout(app: &App, all: bool) -> AppResult<()> {
    let banners: Vec<Banner> = if all {
        Banner::ALL.to_vec()
    } else {
        vec![app.banner]
    };
    for banner in banners {
        if StoredSession::delete(&app.secrets_for(banner), banner)? {
            println!("Forgot the {banner} sign-in.");
        } else {
            println!("Nothing was stored for {banner}.");
        }
    }
    Ok(())
}
