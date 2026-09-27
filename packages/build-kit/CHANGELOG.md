# Changelog

## build-kit/v0.3.1 (2026-09-27)

### Dependencies

- net-kit: 0.1.2 -> 0.2.0


## build-kit/v0.3.0 (2026-09-11)

### Features

- rename repository to shopping-cli-tools

### Dependencies

- net-kit: 0.1.0 -> 0.1.1


## build-kit/v0.2.0 (2026-09-04)

### Features

- show release notes in update --check
  A version number alone is not enough to decide whether to take an update, and
  the release body was being dropped at the wire type.

  `changelog()` returns every release being crossed, newest first, skipping
  previews a stable build was never offered; a downgrade answers with the notes
  of the version asked about. Under --json it is a `changelog` array.


## build-kit/v0.1.0 (2026-09-03)

### Features

- expose each crate's own version
  One subject for seven scopes because it is one line in each, and the reason
  is the same everywhere: `env!` expands where it is written, so a consumer
  asking for `CARGO_PKG_VERSION` gets its own version back. A crate that wants
  to report what it was built against has to be told by the crate itself.

- build provenance and self-update
  Both existing CLIs carry a byte-identical copy of this, differing only in
  an env prefix and a tag namespace.

  env! expands in the crate where it is written, so a library can never read
  a consumer's build-script stamps. The emitter writes a Rust source file
  into the consumer's OUT_DIR and the consumer includes it; OUT_DIR is
  per-crate, so the env! resolves in the right place. Stamp is then an
  ordinary struct, and version strings and dates are testable without
  compiling a binary to test them against.

  install() takes the file to replace instead of reading current_exe
  internally. That turns staging, checksum verification, extraction, mode
  preservation and the rename into unit tests over a temp directory --
  32 tests here against the originals' handful.

  Also adds the repo's first root dependencies: block. The library-to-
  library edge is the one that is easy to forget: without it, releasing
  net-kit rewrites only an app's range and leaves build-kit pinned to a
  version that no longer exists.

### Dependencies

- net-kit: 0.0.0 -> 0.1.0
