# mitre10-api

Mitre 10 New Zealand as one client: catalogue, search, browse, stores, per-store
stock, cart, wishlist and orders.

> Reverse-engineered from the site's own traffic. There is no public API and no
> documentation; these endpoints can change without notice.

## Two surfaces

The storefront is SAP Commerce Cloud (Hybris) with the OCC v2 REST API on
basesite `mitre10`, so most of it is conventional. What is not:

| Surface | Answers | Needs a token |
| --- | --- | --- |
| Algolia | search, browse, suggestions | no |
| OCC — catalogue, stores, stock, cart | everything else | no |
| OCC — account, orders, wishlist | what is filed against a person | **yes** |

Almost everything is anonymous. A cart can be built, priced and switched
between collection and delivery without an account.

```
https://ccapi.mitre10.co.nz/occ/v2/mitre10/...          the storefront
https://<app-id>-dsn.algolia.net/1/indexes/*/queries    the grids
```

The Algolia application id and its search-only key are not held here. The
storefront reads them at boot from an OCC config property, and so does this
crate — one call before the first query, then cached:

```
GET /occ/v2/mitre10/config/key?key=mitre10.algolia.index.config
```

`Client::algolia()` is that call. `Endpoints::with_algolia()` seeds the values
instead, which is how the tests skip it.

## Browse is a filter string, not an endpoint

There is no "products in category" call. Every grid on the site is an Algolia
query, and browsing is the same query with a category filter instead of a term:

```
filters = online:true
          AND (clickAndCollect:66 OR homeDelivery:5 OR expressDelivery:5)
          AND categoryID.lvl2:RF7336
```

**A category code says its own depth.** The prefix letter is the level, so a
bare code is enough and no lookup is needed:

| Prefix | Index field | Example |
| --- | --- | --- |
| `RD` | `categoryID.lvl0` | RD8002 — department |
| `RS` | `categoryID.lvl1` | RS2097 — section |
| `RF` | `categoryID.lvl2` | RF7096 — fineline |
| `RC` | `categoryID.lvl3` | RC37340 — class |

`N1`–`N9` are the top menu entries. They are content pages rather than
catalogue categories, so they have no level and `Query::category` filters
nothing rather than guessing — see `catalogue::browsable`.

The `homeDelivery:5` is a postcode **group**, not a postcode;
`Client::postcode_group` resolves one.

## The category tree lives in the CMS

`/categories/{code}` returns one category and its ancestors but never its
children. The hierarchy is a `CategoryNavigationComponent` embedded in every
page response, whose nodes point at `LN_<code>_<Name>` link components — so the
code and a serviceable name come out of the uids alone, and resolving the
components adds the site's own names and URLs.

`Client::category_tree` does both. The page is around a megabyte, so cache it.

## Stock is one call for every shop

```
GET /products/{code}/stockIndicator
→ [{ stockIndicator: "LOW_STOCK", storeCode: "66", storeDisplayName: "…" }, …]
```

No store selection, no session. The words are the storefront's own vocabulary
and carry no count; `/products/{code}` does carry a number, but only for the
preferred store.

**A store has two codes.** `staticStoreCode` is numeric (`66`) and is what every
call here takes — the Algolia filters, `stockIndicator`, the cart's `storeId`.
`storeCode` is the SAP one (`X57`) and appears only as a product's
`preferredStoreId`. `Store::code` is the numeric one.

## Signing in

OAuth2 authorization code with PKCE against a public client, so there is no
client secret and **no captcha** anywhere in the flow:

1. `GET /authorizationserver/oauth/authorize` — priming; establishes the OAuth
   client in the session and yields no code while signed out
2. `GET /authorizationserver/csrf` — a token and the `JSESSIONID`
3. `POST /authorizationserver/login` — the password, as a form
4. `GET /authorizationserver/oauth/authorize` — the same challenge, now `302`
   with the code in the `Location`
5. `POST /authorizationserver/oauth/token` — code plus verifier, for tokens

Steps 1–4 share one session cookie and run with redirects **off**: following
step 4 discards the code. Access tokens last three hours and refresh without a
password, so only the refresh token is worth storing.

**Steps 1 and 3 are navigations, and have to look like it.** This is the one
trap in the flow. Sending the site's XHR headers on the authorize call —
`Accept: application/json` in particular — gets the *same* `302` to the login
page, so it looks like it worked. But the session it leaves carries no OAuth
client, and `/csrf` then refuses that session:

```
403 Invalid CORS request
```

Three calls later, with nothing pointing back at the cause, and reading as a
CORS or credential problem when it is neither: Spring builds its allowed-origin
list from the OAuth client registration, so "no client in this session" comes
out as a CORS refusal. `navigation_headers` asks for HTML and, like a browser,
sends an `Origin` on the form post and none on the GET.

**`ROUTE` is load-balancer stickiness**, not decoration. The session lives on
one pod, and the same good `JSESSIONID` sent without its `ROUTE` is a `403`
from the same line. The cookie jar carries the whole flow for this reason.

[`Error::LoginUnavailable`] still reports that `403`, because if priming ever
breaks again this is what it will look like.

Everything except the wishlist and past orders is anonymous and unaffected.

## What the API insists on

`Origin: https://www.mitre10.co.nz` and a matching `Referer` on every call.
Without them it answers `403 Invalid CORS request`, which reads as an auth
failure — `Error::Cors` exists to keep those apart.

Cloudflare fronts the API but sets only `__cf_bm`; no managed challenge was
observed on any endpoint here, so the emulation profile is insurance rather
than load-bearing.

## The cart has three traps

**Quantities are whole numbers.** `"quantity": 1.0` is refused with `Request
body is invalid or missing`, which names neither the field nor the reason.
`CartLine::quantity` is an `i64` so a float cannot be formed.

**A cart has two handles and they are not interchangeable.** Anonymous paths
take the `guid`, signed-in paths the `code`, and both are present on both
carts -- so nothing about a cart says which to use. The wrong one answers
`Cart not found`, which reads as an expired basket. `Client::cart_id` knows the
session; `Cart::id_for` takes the answer explicitly.

**A refusal can arrive on a `200`.** Asking for more than a shop holds caps the
line and says so in `errorMessage`, with a `2xx` and a cart that simply is not
what was asked for. Dropped, the command looks like a silent no-op -- so every
mutating call returns a [`CartChange`] carrying the cart *and* the notice.

Display names also arrive with broken entities: the collect mode is literally
`Click &#38 Collect`, double-encoded and missing its terminator. `wire::entities`
decodes them.

## Verified against the live site

Search, browse and suggestions; product, per-store stock, the batch price
lookup, stores, the postcode group, and the 1,094-node category tree. The cart
is implemented from the captures and not exercised against a live basket.

Two things the captures did not show, both caught only by running it:

* **OCC serves XML unless asked otherwise.** With no `Accept` header it answers
  `200 application/xml`, which fails as a decode error and reads as a moved
  schema. Every request here carries the site's own `Accept`.
* **The menu has structural wrapper nodes.** Every department hangs off
  `Mitre10DepartmentNavNode`, which has no category code — dropping it dropped
  the entire tree.

## Not confirmed

Only the `relevance` index is confirmed against a capture. Algolia replicas are
conventionally `<index>_<order>`, so price and name orderings very likely exist,
but an index that does not answers `404` — hence `Sort::Index` rather than a
list of guesses. Checkout and payment are not implemented; the captures stop at
the cart.

## Usage

```rust
let http = net_kit::http::build(mitre10_api::client_spec())?;
let client = Client::new(http, Endpoints::defaults(), Session::default());

let listing = client.listing(&Query::category("RF7336")).await?;
let stock = client.stock("174969").await?;
```

Nothing here reads the environment; `clippy.toml` enforces it. The app reads it
once and passes values down.
