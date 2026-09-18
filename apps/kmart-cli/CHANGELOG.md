# Changelog

## kmart-cli/v0.2.1 (2026-09-18)

### Fixes

- stop reading the password on every command
  The client was built with the password already fetched, so every command
  paid a keychain prompt for a renewal that almost never happens. It now
  carries where the password is. auth status and auth refresh still ask
  outright, because that is what they report on.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2
- kmart-api: 0.2.0 -> 0.2.1


## kmart-cli/v0.2.0 (2026-09-11)

### Features

- add auth refresh, for signing in with nobody watching
  A session is two credentials that fail differently: an Auth0 token, and
  Akamai's `_abck`, which lasts about a day. Renewing the token and
  reporting success left the shorter half untouched -- the exact failure
  the command exists to prevent. `_abck` carries no readable clock, a
  stale one being byte-identical to a good one, so the only honest test is
  to spend a gateway request; that probe is now the pivot. Cheapest first:
  a token with time on it is left alone unless `--force`, since every use
  of a refresh token rotates it and a cron tick should not.

  Renewal runs under the Auth0 application that minted the session rather
  than whichever country is selected now -- the two are separate
  applications -- and the browser fallback signs in against that origin so
  `auth_country` survives. Nothing to renew exits 3 rather than succeeding
  quietly, and says which half is missing.

  The default country is Australia, and `auth login` writes the country it
  signed in as to the config, so an unattended renewal repeats the same
  way. It is the one command where `--country` is not per-command.

  `press` in the browser script now reports which of three failures it hit
  -- nothing matched, matches all invisible, a visible element whose click
  was refused -- where `except Exception: continue` had been discarding
  Playwright's own explanation. A synthetic click is the last resort,
  which is also the answer to an overlay, one having been seen over the
  header on a run.

### Dependencies

- build-kit: 0.2.0 -> 0.3.0
- net-kit: 0.1.0 -> 0.1.1
- kmart-api: 0.1.0 -> 0.2.0


## kmart-cli/v0.1.0 (2026-09-05)

### Features

- default to headless, add --direct sign-in
  Headless passes the bot check in the common case, so it is the default;
  `--headful` shows the window and is the stronger path when a headless
  run is refused. `--direct` signs in with no browser at all, replaying
  Auth0's login as direct requests and seeding any admission the session
  holds -- a diagnostic that measures the Akamai wall and would notice the
  day it moves, not a way in. Refresh the now-stale "headless was refused"
  notes to match.

- add kmart, for the Australian and New Zealand stores
  Command-for-command what `twlnz` does, over `kmart-api`. One binary for
  both countries because they share a backend: `kmart use au` sets which
  one, `--country` overrides it for a command.

  `auth login` drives camoufox, because Akamai guards the password submit
  and refuses every HTTP client. It navigates the way a person would --
  home page, then the account link -- since asking for a protected path
  outright is answered with an interstitial that never resolves, and it
  reads the refresh token out of the storefront's own storage rather than
  running a second copy of the OAuth flow. `auth token` and `auth import`
  remain for when there is no browser to hand.

  Wishlist removal is missing: the gateway's add is a bespoke field rather
  than an action list, introspection is off, and no capture of a removal
  exists to name the operation.

  Vendor identifiers are re-read from the storefront about weekly and cached in
  the state directory, falling back to what shipped when it cannot be reached;
  `doctor` reports which is in force.

### Dependencies

- kmart-api: 0.0.0 -> 0.1.0
