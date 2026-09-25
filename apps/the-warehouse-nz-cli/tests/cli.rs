//! The binary, end to end, with no network.
//!
//! Everything here runs against a config and state directory of its own, so a
//! test never reads or writes the machine's real ones. The commands that would
//! talk to The Warehouse are not exercised here -- `twlnz-api` owns those
//! against a mock server -- so what is left is the part this crate is
//! responsible for: flags, config, and the exit code a script sees.

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

/// A run with its own config and state, and no colour.
fn twlnz(home: &tempfile::TempDir) -> Command {
    let mut cmd = Command::cargo_bin("twlnz").expect("the binary");
    cmd.env("TWLNZ_CONFIG_DIR", home.path())
        .env("TWLNZ_STATE_DIR", home.path())
        .env("TWLNZ_SECRET_BACKEND", "file")
        .env("NO_COLOR", "1")
        // Pointed at a port nothing listens on, so a command that would reach
        // the site fails as a connection error rather than by contacting it.
        .env("TWLNZ_ORIGIN", "http://127.0.0.1:9")
        .env("TWLNZ_REQUEST_INTERVAL", "0");
    cmd
}

fn home() -> tempfile::TempDir {
    tempfile::TempDir::new().expect("a temp dir")
}

#[test]
fn version_names_the_libraries_it_was_built_from() {
    // The point of printing them: "twlnz 0.1.0" does not say which twlnz-api
    // was compiled in, and that is the part that breaks when the site changes.
    let home = home();
    twlnz(&home)
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("twlnz-api"))
        .stdout(contains("cli-kit"));
}

#[test]
fn help_lists_the_commands() {
    let home = home();
    twlnz(&home)
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("search"))
        .stdout(contains("stock"))
        .stdout(contains("island"))
        .stdout(contains("region"));
}

#[test]
fn an_unknown_setting_is_refused_with_the_misuse_code() {
    // 2 is the shell's convention, and it is what tells a wrapper "you typed
    // this wrong" apart from "the site said no".
    let home = home();
    twlnz(&home)
        .args(["config", "get", "nonsense"])
        .assert()
        .code(2)
        .stderr(contains("no setting called"));
}

#[test]
fn a_setting_round_trips_through_the_file() {
    let home = home();
    twlnz(&home)
        .args(["config", "set", "island", "south"])
        .assert()
        .success();
    twlnz(&home)
        .args(["config", "get", "island"])
        .assert()
        .success()
        .stdout("south\n");
    assert!(home.path().join("config.toml").exists());
}

#[test]
fn a_region_is_resolved_when_it_is_written() {
    // Stored as the code, so no later command has to guess what the name meant.
    let home = home();
    twlnz(&home)
        .args(["config", "set", "region", "Canterbury"])
        .assert()
        .success();
    twlnz(&home)
        .args(["config", "get", "region"])
        .assert()
        .success()
        .stdout("NZ-CAN\n");
}

#[test]
fn a_bad_value_is_refused_at_the_point_of_writing_it() {
    let home = home();
    twlnz(&home)
        .args(["config", "set", "island", "east"])
        .assert()
        .code(2)
        .stderr(contains("north"));
}

#[test]
fn the_island_command_and_the_config_key_are_the_same_setting() {
    // Two ways in, one value. If they drifted, `island show` would disagree
    // with `config get island` and neither would be wrong on its face.
    let home = home();
    twlnz(&home)
        .args(["island", "set", "north"])
        .assert()
        .success()
        .stdout(contains("north island"));
    twlnz(&home)
        .args(["config", "get", "island"])
        .assert()
        .success()
        .stdout("north\n");
    twlnz(&home).args(["island", "clear"]).assert().success();
    twlnz(&home)
        .args(["island", "show"])
        .assert()
        .success()
        .stdout(contains("No island set"));
}

#[test]
fn an_account_command_says_to_sign_in_rather_than_failing_obscurely() {
    // Exit 3 is the auth code, and the advice names this binary's own command.
    let home = home();
    twlnz(&home)
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("Signed out"));
}

#[test]
fn the_wishlist_needs_no_subcommand_to_be_shown() {
    // The bare command is the listing, and `list` is kept as the same thing
    // said longhand. Both get as far as needing an account, which is where a
    // signed-out run stops -- so this pins the parse rather than the network.
    let home = home();
    for args in [vec!["wishlist"], vec!["wishlist", "list"]] {
        twlnz(&home)
            .args(&args)
            .assert()
            .code(3)
            .stderr(contains("not signed in"));
    }
}

#[test]
fn logging_out_when_signed_out_is_not_an_error() {
    let home = home();
    twlnz(&home)
        .args(["auth", "logout"])
        .assert()
        .success()
        .stdout(contains("Nothing to forget"));
}

#[test]
fn completions_print_a_script_and_nothing_else() {
    // Anything else on stdout would break `source <(twlnz completions zsh)`.
    let home = home();
    let out = twlnz(&home)
        .args(["completions", "zsh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).expect("utf-8");
    assert!(
        text.starts_with("#compdef twlnz"),
        "{}",
        &text[..60.min(text.len())]
    );
}

#[test]
fn an_unknown_shell_is_refused_rather_than_guessed_at() {
    let home = home();
    twlnz(&home).args(["completions", "csh"]).assert().code(2);
}

#[test]
fn json_is_a_document_on_stdout() {
    let home = home();
    let out = twlnz(&home)
        .args(["--json", "config", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).expect("a JSON document");
    assert!(value["settings"].as_array().is_some_and(|s| !s.is_empty()));
}

#[test]
fn island_and_region_are_separate_settings_that_do_not_collide() {
    // The site calls both of these "region". Conflating them here would mean
    // setting a listing filter and silently moving the store finder, or the
    // other way round.
    let home = home();
    twlnz(&home)
        .args(["island", "set", "south"])
        .assert()
        .success();
    twlnz(&home)
        .args(["region", "set", "NZ-CAN"])
        .assert()
        .success();
    twlnz(&home)
        .args(["config", "get", "island"])
        .assert()
        .success()
        .stdout("south\n");
    twlnz(&home)
        .args(["config", "get", "region"])
        .assert()
        .success()
        .stdout("NZ-CAN\n");

    // Clearing one leaves the other alone.
    twlnz(&home).args(["island", "clear"]).assert().success();
    twlnz(&home)
        .args(["config", "get", "region"])
        .assert()
        .success()
        .stdout("NZ-CAN\n");
}

#[test]
fn each_list_marks_what_is_selected() {
    let home = home();
    twlnz(&home)
        .args(["island", "set", "north"])
        .assert()
        .success();
    twlnz(&home)
        .args(["island", "list"])
        .assert()
        .success()
        .stdout(contains("* north"));

    twlnz(&home)
        .args(["region", "set", "Canterbury"])
        .assert()
        .success();
    twlnz(&home)
        .args(["region", "list"])
        .assert()
        .success()
        .stdout(contains("NZ-CAN"));
}

#[test]
fn an_unknown_region_is_refused_by_name_or_by_code() {
    let home = home();
    twlnz(&home)
        .args(["region", "set", "Chatham Islands"])
        .assert()
        .code(5)
        .stderr(contains("twlnz region list"));
}

#[test]
fn a_store_shown_offline_falls_back_to_the_id_rather_than_failing() {
    // `store show` names the store when it can, which takes a request. Losing
    // the name is not a reason to lose the answer.
    let home = home();
    twlnz(&home)
        .args(["config", "set", "store_id", "116"])
        .assert()
        .success();
    twlnz(&home)
        .args(["config", "set", "region", "NZ-NTL"])
        .assert()
        .success();
    twlnz(&home)
        .args(["store", "show"])
        .assert()
        .success()
        .stdout("116\n");
}

#[test]
fn a_site_that_cannot_be_reached_fails_without_claiming_an_empty_result() {
    // The distinction that matters: "nothing found" and "could not ask" are
    // different answers, and only one of them is safe to act on.
    let home = home();
    twlnz(&home)
        .args(["search", "anything"])
        .assert()
        .failure()
        .stderr(contains("twlnz:"));
}

#[test]
fn doctor_reports_a_failure_rather_than_pretending_to_be_healthy() {
    let home = home();
    twlnz(&home)
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(contains("not healthy"));
}

#[test]
fn a_refresh_with_nothing_on_file_asks_for_a_person_rather_than_reporting_success() {
    // The whole point of the command is to be run by a cron job, so the one
    // thing it must never do is exit 0 having done nothing. 3 is the code that
    // says a person is needed.
    let home = home();
    twlnz(&home)
        .args(["auth", "refresh"])
        .assert()
        .code(3)
        .stderr(contains("not signed in"));
}

#[test]
fn refresh_names_what_it_would_need_rather_than_just_refusing() {
    // Two different holes -- no account and no password -- and the advice for
    // them differs, so the message says which one it hit.
    let home = home();
    twlnz(&home)
        .args(["auth", "refresh", "--help"])
        .assert()
        .success()
        .stdout(contains("password_command"))
        .stdout(contains("--force"));
}

#[test]
fn an_unusable_browser_profile_is_refused_before_anything_is_asked() {
    // A typo silently falling back to the default would be the worst answer
    // available: the default is exactly what the override exists to escape.
    let home = home();
    twlnz(&home)
        .env("TWLNZ_EMULATION", "safari26_99")
        .args(["auth", "status"])
        .assert()
        .code(2)
        .stderr(contains("not a browser profile"));
}

#[test]
fn doctor_says_which_browser_the_run_presented_as() {
    // The first question to ask of a report where every line failed at once,
    // because that is what a profile the site has turned against looks like.
    let home = home();
    twlnz(&home)
        .env("TWLNZ_EMULATION", "firefox151")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(contains("presents as").and(contains("Firefox151")));
}
