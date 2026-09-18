# Changelog

## briscoe-group-nz-cli/v0.1.1 (2026-09-18)

### Fixes

- run the password command directly
  It went through a password source, which now answers with an Option
  because a stored password may not be there. Running the configured
  command is not that question.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2


## briscoe-group-nz-cli/v0.1.0 (2026-09-11)

### Features

- add bgnz, the Briscoes and Rebel Sport CLI
  `bgnz` — search either catalogue, read a product, check click-and-collect
  stock, and — signed in — work the cart, wishlist, orders and loyalty
  balance. Everything takes `--json`.

  One backend serves both fascias behind a `store` header, so `-b` is a
  switch, not a fan-out: the SKU namespaces are disjoint, so there is no
  `compare` here, and with separate Gigya sites the credentials are filed
  per fascia — `auth status` always shows both so that stays visible.

  The parts worth noting:

  - Stock is per size. A configurable product has no barcode of its own,
    only its variants do, and the collection service works by barcode —
    so `stock` on a shoe answers per variant, and `cart collect` gives one
    verdict for the whole basket because that is what the service answers.
  - Two order histories, neither a page of the other: `orders list` is
    online purchases with carrier tracking, `orders receipts` is what was
    bought standing in a shop, out of SAP, sharing no identifiers.
  - `auth login` drives a browser only to mint the reCAPTCHA token Gigya
    demands before checking the password — the credentials never reach
    the browser, and the sign-in request is made here. Renewal afterwards
    needs no browser. `auth token --cookie` skips the browser entirely.

  The protocol lives in `bgnz-api`; the `doctor`/`auth status` shape and
  the verdict vocabulary come from `cli-kit`. Ships binaries for
  ubuntu-latest and macos-14.

### Dependencies

- build-kit: 0.3.0 -> 0.3.0
- cli-kit: 0.2.0 -> 0.3.0
- bgnz-api: 0.0.0 -> 0.1.0
