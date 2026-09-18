# farmers

An unofficial command line tool for [Farmers](https://www.farmers.co.nz) New
Zealand. Search the catalogue, price a product, and find which stores have it.

> Not affiliated with Farmers. Reverse-engineered from the site's own traffic;
> the endpoints it uses can change without notice.

```
farmers search "fleece robe"            what the site's search finds
farmers search robe --prices --facets   with prices, and the filters on offer
farmers browse 51-0303                  a category, by id
farmers categories men                  the tree, searched
farmers product 6867065002              one product
farmers product 6867065 --variants      a master, and the sizes under it
farmers stock 6867065002 --region AUK   which Auckland stores have it
farmers prices < codes.txt              a batch, one code per line
farmers cart list                       the basket
farmers cart add 6867065002 -q 2        put something in
farmers cart remove 1                   take line 1 out
farmers orders                          what has been bought
farmers wishlist list                   saved lists
farmers auth login                      sign in
```

`--json` on any command prints the same data as a document instead of a table.

## What a product code is

`6867065` is a **master** — a price range, no stock of its own, and it cannot
be bought. `6867065002` is the size that can be. Search returns masters, so
`farmers product` on one lists its variants unasked; the code in the first
column is what to buy.

Anything that looks like a product code is accepted: a bare code, a pasted URL,
or the `…-6867065|6867065001|6867065002` form the address bar holds.

## Search has no prices in it

The search backend is a different service from the catalogue and carries no
price at all. `--prices` looks each one up, at one extra request per product,
which is why it is a flag rather than the default.

## The basket

Anonymous — it hangs off the session, works signed out, and survives signing in
afterwards. Lines are addressed by the number `cart list` shows, because the
real ids are opaque 24-character strings:

```
farmers cart list
farmers cart set 1 3        line 1 to three of them
farmers cart remove 2
farmers cart promo SAVE10
```

`cart add` refuses a master code and lists the variants to pick from, because
the storefront answers an add for a master by silently doing nothing.

The total excludes delivery, which Farmers works out at checkout. `cart promo`
reports whether the total actually moved rather than claiming a code applied,
since the storefront does not say so in any readable way.

## Orders and saved lists

Both need an account, and both exit **3** signed out without touching the
network.

Farmers keeps *several* saved lists per account rather than one, and
`wishlist add` writes to whichever is marked preferred. There is no
`wishlist remove`: the storefront has no call that takes one item back off a
list.

One honest caveat: the account these were reverse-engineered from had no orders
and no saved lists, so the **empty** states are verified against real markup and
the populated table layouts are inferred from the same template's class names.
If `farmers orders` ever shows an order with blank columns, that is why, and it
is worth reporting.

## Bot protection

Farmers sits behind Akamai, and **it only admits a client whose cookies a real
browser earned**. So this tool drives one: the first command that touches the
catalogue starts [camoufox](https://camoufox.com), loads one page, keeps the
cookies and does everything else over HTTP. That costs about ten seconds, once;
the commands after it reuse the jar and take no longer than they used to.

```
uv tool install "camoufox[geoip]" && camoufox fetch
```

`farmers doctor` says whether it can find one. Without it, search and suggest
still work — they go to a different host — and everything else is refused.

The jar lapses after a few minutes and a couple of dozen requests, with no
warning: the request that used it a moment ago starts getting `Access Denied`.
A refused command starts the browser again and retries once, so this is
usually invisible. When a headless run is what is being refused, `--headful`
shows the window and gives the bot manager more to score.

Measured 2026-09-18, on one address inside one minute: a jar harvested from
camoufox was served the whole category tree through this tool's own HTTP
client, and a jar that client earned from the same home page got `Access
Denied` from the byte-identical request. So waiting does **not** help, and
neither does `FMNZ_EMULATION` — what is read is where the cookie came from,
not what the request looks like.

`cart add` is the one command with a second helping of this: the storefront
guards its add-to-cart POST harder than anything else and refuses it even on a
good jar. The same add sent as a GET is served, which is what this tool does,
so the command works like the rest of them.

One refusal shape is worth knowing about because it used to pass silently: an
Akamai interstitial that arrives as `200` with a sensor widget for a body. It
looks like an empty page, so `farmers orders` once printed "No orders." for an
account whose history had simply not been served. It is detected now and
reported as a refusal, but it is the reason an empty result from a gated
command is worth a second look. `farmers search` and `farmers suggest` go to a different
host entirely and keep working even when everything else is refused.

Requests are still kept few, because there is no reason to be noisy. The tool
warms once per run and carries the result to the next; it retries a refusal
exactly once and then stops; and nothing runs in parallel. A bare
`farmers stock` is thirteen requests — one per region — so prefer `--region`,
or set one:

```
farmers config set region Auckland
```

When it does refuse, the exit code is **8** and the message says so. It is not
a rate limit and not a sign-in problem, so neither waiting nor signing in is
the answer — a browser is. `farmers search` and `farmers suggest` keep working
throughout, because they go somewhere else entirely.

## Signing in

An email and a password; there is no captcha.

```
farmers auth login
farmers auth status
farmers auth refresh     sign in again, unattended
farmers auth logout      forgets the session and the password
```

`auth status` and `auth refresh` ask the storefront rather than trusting the
cookie, so they catch a session that has been dropped at the other end.

Signing in is not needed for anything above — search, browse, product, stock
and prices are all anonymous.

### Staying signed in

There is no refresh token here. The session is a cookie and the only way to get
another is to run the sign-in form again, so **unattended renewal needs the
password**. `auth login` keeps it by default, in the system credential store
where there is one:

```
farmers config set auth.store_password false   don't keep it
```

Better, if you have a password manager — the command wins over the stored copy,
so nothing is written and the manager stays the only place it lives:

```
farmers config set auth.password_command "pass show farmers"
```

`auth refresh` is the one to put on a timer. It checks whether it needs to run
at all, signs in again only if the session has actually stopped working, and
exits **3** if there is nothing on file to sign in with — so a schedule that has
quietly stopped working shows up as a failure rather than as a success that did
nothing.

One thing it deliberately does not do: if the bot manager refuses to say whether
the session is good, `refresh` stops with exit 8 rather than signing in again.
The login form would be refused identically, so trying would spend the password
to learn nothing and would report a credential problem that does not exist.

## Exit codes

| | |
| --- | --- |
| 0 | it worked |
| 1 | something else went wrong |
| 2 | a flag or a setting this tool refused |
| 3 | not signed in, or the password was rejected |
| 5 | no such product, category or region |
| 8 | refused by Farmers' bot protection |

## Settings

```
farmers config list
farmers config set region Canterbury
farmers config set page_size 48
farmers config set auth.password_command "pass show farmers"
```

| Variable | |
| --- | --- |
| `FMNZ_CONFIG_DIR`, `FMNZ_STATE_DIR` | where this tool keeps its files |
| `FMNZ_SECRET_BACKEND` | `keyring` or `file` |
| `FMNZ_DEBUG` | narrate the requests on stderr — URLs and step names, never a credential |
| `FMNZ_EMULATION` | the browser profile to present as |
| `FMNZ_BROWSER_PYTHON` | the Python that can `import camoufox`, if the `camoufox` launcher names the wrong one |
| `FMNZ_HEADFUL` | show the warm-up browser's window, as `--headful` does |
| `FMNZ_SITE_ORIGIN`, `FMNZ_SEARCH_ORIGIN` | point it at a mock |

## Building

```
dispat run check --since all -p farmers-nz-cli
```

The protocol lives in [`farmers-api`](../../packages/farmers-api); this crate is
flags, config and rendering.
