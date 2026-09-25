# the-warehouse-nz-cli

`twlnz` — search and shop [The Warehouse](https://www.thewarehouse.co.nz) New
Zealand from the terminal.

> **Not affiliated with The Warehouse.** There is no public API. This calls the
> same undocumented endpoints their website calls from the browser, and can
> break whenever they change something. Use at your own risk.

## How it is built

`twlnz` is a thin front end over the crates in `packages/`:

| Crate | What it holds |
| --- | --- |
| [`twlnz-api`](../../packages/twlnz-api) | the storefront: listings, products, cart, wishlist, stores — in its own vendor-shaped types |
| [`cli-kit`](../../packages/cli-kit) | tables, `--json`, prompts, `doctor`, completions |
| [`net-kit`](../../packages/net-kit) | the browser-fingerprinted HTTP client, cookies and the credential store |
| [`build-kit`](../../packages/build-kit) | the build stamp and `twlnz update` |

What is left in `src/` is reading the environment once, resolving flags against
config, rendering, and turning a failure into an exit code.

It does **not** build on `gsnz-core` and `gsnz-ui`, and there is no Warehouse
adapter in [`gsnz`](../grocery-nz-cli): `gsnz_core::Product` has a `SaleUnit`
and nowhere to put a colour or size axis, and this catalogue is full of
variation masters. `cli-kit` carries no domain, so the shared half comes along
anyway; the cost is the `View` impls in [`src/views/`](src/views).

## Install

```bash
cargo build                        # from this directory
cargo install --path .             # or install the `twlnz` binary
```

Or take a published build from
[releases](https://github.com/jason-s13r/shopping-cli-tools/releases), tagged
`the-warehouse-nz-cli/vX.Y.Z`. Once installed the binary replaces itself:

```bash
twlnz update --check     # is there a newer one, and what changed in it?
twlnz update             # download it and swap it in
```

## Use

```bash
twlnz search "lego" --limit 10
twlnz search "tee" --brand "H&H" --color "Blue Dark" --sort price-low-to-high
twlnz departments                       # the category tree, with the ids `browse` takes
twlnz browse toysbaby --limit 20
twlnz specials

twlnz product RM110166766-10M           # price, variations, per-channel stock
twlnz product RM110166766-10M --select size=XL
twlnz stock R3035996 --region canterbury

twlnz stores whangarei                  # searched nationwide
twlnz stores --region canterbury        # or one region, listed in full
twlnz store set 116                     # found anywhere, no region needed

twlnz island set south                  # north/south: what a listing contains
twlnz region set canterbury             # NZ-CAN: which shops get asked

twlnz auth login
twlnz auth refresh                      # sign in again, unattended
twlnz cart add R3059518 2
twlnz cart list

twlnz wishlist                          # what is saved
twlnz wishlist add R3059518
twlnz wishlist set R3059518 2           # how many are wanted; 0 stops saving it
twlnz wishlist move-to-cart R3059518
```

Every command takes `--json`, built from the same data as the table.

### `island` and `region` are different things

The site calls both "region". Both are `show` / `list` / `set` / `clear`.

| | `twlnz island` | `twlnz region` |
| --- | --- | --- |
| Values | `north`, `south` | the sixteen `NZ-` codes |
| Decides | **what a listing contains** | **which shops get asked** |
| Used by | `search`, `browse`, `specials` | `stores`, `stock` |
| Override for one run | `--island` | `--region` |

The island is not cosmetic: The Warehouse ranges differently north and south, so
a product absent from one island's results can be on the shelf on the other.

### Finding a store

`twlnz stores <name>` searches **nationwide**. The finder is per region and
there is no call that lists them all, so the whole directory — about 84 shops —
is fetched once, all sixteen regions at a time, and cached for a week. After
that it is instant and works offline; `--refresh` re-fetches it, and `twlnz
doctor` says how old it is. The sixteen lookups go out four at a time, which is
roughly what a browser opens to one host.

With no name to search for it lists one region instead.

`store set` uses the same directory, so an id copied out of any listing works
without also saying which region it came from.

### Unattended sign-in

`twlnz auth refresh` is `auth login` without the typing:

```bash
twlnz auth refresh          # exit 0 if the session is good, 3 if it needs you
```

There is one credential and nothing to renew it from — the storefront
authorises by cookie and the cookies come from a form POST — so a session that
has to be replaced is replaced by running the form again. The shopper token is
a readable JWT, so an expired one is spotted for free; a token that still looks
good costs a single request to the account page, because the storefront can
drop a session at its end without the token knowing.

The password it signs in with is `auth.password_command` where you set one,
otherwise the copy `auth login` kept. With neither, it exits 3.

```bash
twlnz config set auth.password_command 'op read "op://Vault/Warehouse/password"'
```

`--force` signs in again whatever state the session is in.

### A cart write costs one extra read

The site answers a write with a partial basket — the lines, and no subtotal or
count — so `cart add` and `cart remove` re-read the minicart before printing,
which is what the site's own page does.

### `wishlist` shows the list without being asked

`twlnz wishlist` prints what is saved; `wishlist list` is accepted too. The rest
are `add`, `remove`, `set` and `move-to-cart`, all by product id — the site
addresses a saved row by a `uuid` a person never sees, so each of these reads
the list first to turn the id into the row.

`move-to-cart` is **two changes**: the product goes into the cart, then off the
list. In that order, so a failure in between leaves it in the cart and still
saved rather than in neither. It defaults to the quantity saved against the
row.

Saving is not buying: the quantity is a note to self, nothing is reserved, and
the site quotes no total — so the table has one price column where `cart` has
two.

### The store is a local preference

`store set` records the store here and pulls its region along, so `stock` and
`stores` default to where it actually is rather than to Auckland. It does not
bind the store server-side: `Cart-SelectStore` needs a basket and answers an
empty one with a 500. That belongs to checking out, which this tool does not
do.

### Stock has two axes

An item can be orderable online, orderable only by walking into a shop, both,
or neither. `twlnz search` prints `in store` for the second, never `sold out`.

```
$ twlnz product RM110166766-10M --select size=XL
H&H Men's Regular Fit Crew Neck Tee
$6.99  in store
Online out of stock
```

### Variations

A listing links to a variation group; the cart takes a variant. `twlnz product`
shows every axis with three states, because "not made in this combination" and
"sold out" send you in different directions:

```
Color (Blue Dark)
    Beige
  * Blue Dark          <- chosen
  x Blue Mid sold out
    Brown Dark
```

## Configuration

`twlnz config list` shows every setting and what it does. Precedence is flag,
then environment, then config file, then the default.

| Variable | What it moves |
| --- | --- |
| `TWLNZ_CONFIG_DIR` / `TWLNZ_STATE_DIR` | where config and state live |
| `TWLNZ_SECRET_BACKEND` | `keyring` or `file` |
| `TWLNZ_ORIGIN` | the storefront, for pointing at a mock server |
| `TWLNZ_EMULATION` | the browser to present as, by `wreq-util` name |
| `TWLNZ_REQUEST_INTERVAL` | seconds between requests (default `1`, `0` for none) |
| `TWLNZ_DEBUG` | narrate requests on stderr — cookie names only, no query strings |
| `NO_COLOR` | honoured whatever the config says |

### Request pacing

Requests are spaced about a second apart. The site answering 429 or 503 is
retried after a Fibonacci backoff (about 1, 1, 2, 3s), and a `Retry-After`
longer than 30s is reported rather than waited out. `TWLNZ_DEBUG` shows each
retry. Change the gap with `TWLNZ_REQUEST_INTERVAL` or:

```bash
twlnz config set network.request_interval 2
```

### When every request 403s at once

Cloudflare sits in front of this storefront and scores the TLS handshake, the
HTTP/2 settings and the headers together, and **which emulation profile is
accepted changes without notice**. As of 2026-09-11 every Safari profile is
served and every Firefox, Chrome and Edge one — newest included — is answered
with a 403 "Just a moment…" challenge on the home page itself. `Firefox151` was
the default until then and worked a week earlier.

A refused profile stops everything, so the symptom is a `twlnz doctor` where
the listing probe fails and nothing works signed in or out. `doctor` prints the
profile it used next to the origin, and `TWLNZ_EMULATION` changes it without
waiting for a release:

```bash
TWLNZ_EMULATION=safari18_5 twlnz doctor
```

Bumping to a newer version of the same browser is not the fix — the split is by
family, not by version.

## Exit codes

| Code | Means |
| --- | --- |
| 0 | fine |
| 1 | something went wrong |
| 2 | you typed it wrong |
| 3 | sign in, or sign in again |
| 5 | no such store or region |
| 7 | the site is rate-limiting; back off |
