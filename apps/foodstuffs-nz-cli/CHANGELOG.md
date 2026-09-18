# Changelog

## foodstuffs-nz-cli/v0.6.2 (2026-09-18)

### Fixes

- stop reading the password on every command
  The client was built with the password already fetched, so every command
  paid a keychain prompt for a renewal that almost never happens. It now
  carries where the password is. auth status and auth refresh still ask
  outright, because that is what they report on.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2
- fsnz-api: 0.2.0 -> 0.2.1


## foodstuffs-nz-cli/v0.6.1 (2026-09-11)

### Dependencies

- build-kit: 0.2.0 -> 0.3.0
- net-kit: 0.1.0 -> 0.1.1


## foodstuffs-nz-cli/v0.6.0 (2026-09-04)

### Features

- show release notes in update --check
  A version number alone is not enough to decide whether to take an update, and
  the release body was being dropped at the wire type.

  `changelog()` returns every release being crossed, newest first, skipping
  previews a stable build was never offered; a downgrade answers with the notes
  of the version asked about. Under --json it is a `changelog` array.

### Fixes

- declare build-kit once
  dispat rewrites one version range per manifest, so the [build-dependencies]
  build-kit stayed at ^0.1.0 when the [dependencies] one moved to ^0.2.0 and the
  release failed to select a version. Latent until build-kit's first bump.

  A path dependency needs a version only to be publishable, so the second
  declaration drops it rather than carrying a number nothing keeps honest.

### Dependencies

- build-kit: 0.1.0 -> 0.2.0


## foodstuffs-nz-cli/v0.5.0 (2026-09-03)

### Features

- rebuild on the shared gsnz libraries
  The hand-rolled HTTP, auth, domain and rendering code is replaced by a Foodstuffs-only fork of grocery-nz-cli's architecture:
  gsnz-core, fsnz-api, cli-kit, gsnz-ui, net-kit and build-kit do the work.
  No wwnz-api, no Woolworths.


## foodstuffs-nz-cli/v0.4.0 (2026-09-02)

### Features

- keep the password for unattended re-login
  The refresh token is rotated and single-use, so a run that spent one and
  died had no way back short of a prompt. `auth login` now keeps the password
  in the credential store and `active_session` signs in again from it once
  the refresh token is gone. It cannot answer a verification code, which
  still needs `fsnz auth login`.

  `--no-store-password` and `store_password = false` opt out; a configured
  `password_command` is preferred over the stored copy.


## foodstuffs-nz-cli/v0.3.1 (2026-09-02)

### Fixes

- satisfy rustfmt check

- bump wreq version and use user tokens across grocery APIs


## foodstuffs-nz-cli/v0.3.0 (2026-09-02)

### Features

- accept the emailed Club Plus verification code
  An unrecognised device gets isTFARequired and a phvToken instead of a session.
  /user/tfa/login redeems the emailed code under the pre-TFA token. No captcha.

  auth login prompts and stores nothing until the code is accepted. Read from
  stdin where there is no terminal.

  401 from /user/token/secure quotes the API instead of blaming a stale session.

### Fixes

- use a browser handshake instead of curl
  Cloudflare scores handshake and protocol together: curl passes on HTTP/1.1 and
  is challenged on h2, a browser fingerprint the reverse. curl and reqwest are
  replaced by wreq. http::EMULATION drives handshake, h2 and headers;
  http1_only() breaks it.

  Cookies are kept and replayed. Cloudflare's, and the storefront's fs-user-token
  and refresh_token, persist beside the login; analytics does not. auth logout
  clears them.

  update.rs sets its own redirect policy; wreq defaults to Policy::none()
  (4da18b6).


## foodstuffs-nz-cli/v0.2.0 (2026-08-31)

### Features

- follow the release channel the running build is on
  `fsnz update` skipped every prerelease, so a preview build was stranded: it
  could not reach the next preview, and would not move until a stable release
  passed it.

  The channel now follows from the running version and is not stored. A stable
  build still only sees stable releases. A preview build takes a newer stable
  when one exists, and otherwise walks on through the previews, so it rejoins the
  stable channel by itself.

  `--pre-release` takes the newest release of either channel. A version argument
  installs exactly that release, downgrades included, with the leading `v`
  optional; it pins nothing, so the next plain update follows the rules above.
  `--check` mentions a preview without counting it, so it cannot flip the exit
  code a script gates on.

### Fixes

- keep Cargo.lock in step with the released version
  The release job never runs cargo -- its build stage collects what the matrix
  runners already built -- so nothing rewrote the lock after the version stage
  bumped the manifest, and the release commit had no lock change to carry. main
  drifted a release further each time.

  autoVersion.syncLock runs between the version and build stages, which is where
  that regeneration belongs. `cargo metadata` rewrites the package's own version
  and nothing else. Offline first, since a warm cache makes it free; a fresh
  runner has no registry to read and falls back to the network.

- drop the dirty marker from the build stamp
  A release build rewrites Cargo.lock as it runs, so whether `git status` saw the
  tree clean depended on whether it ran before or after cargo got there. Two
  releases off the same pipeline disagreed, and 0.2.0-rc.1 shipped branding
  itself as built from edited source.

  `built by` already separates a released binary from a hand-built one, which is
  the distinction that carries weight.

- cut a candidate to exercise self-update

- stamp the released version into the binary
  The build matrix runs `dispat run`, which has no version stage, so the
  checked-out Cargo.toml still carries the previous release. Every published
  binary reported it: the 0.1.4-rc.0 tarball holds a binary saying 0.1.3, with
  no release tag. build.rs now prefers DISPAT_NEW_VERSION and DISPAT_TAG,
  falling back to the manifest and `git describe` outside a release.

  The update test mounted its "already newest" release at the running version,
  which a prerelease build filters back out; it now also mounts an older stable
  release, so it holds on either channel.

- exercise the dispat release pipeline


## foodstuffs-nz-cli/v0.2.0-rc.3 (2026-08-31)

### Fixes

- keep Cargo.lock in step with the released version
  The release job never runs cargo -- its build stage collects what the matrix
  runners already built -- so nothing rewrote the lock after the version stage
  bumped the manifest, and the release commit had no lock change to carry. main
  drifted a release further each time.

  autoVersion.syncLock runs between the version and build stages, which is where
  that regeneration belongs. `cargo metadata` rewrites the package's own version
  and nothing else. Offline first, since a warm cache makes it free; a fresh
  runner has no registry to read and falls back to the network.


## foodstuffs-nz-cli/v0.2.0-rc.2 (2026-08-31)

### Fixes

- drop the dirty marker from the build stamp
  A release build rewrites Cargo.lock as it runs, so whether `git status` saw the
  tree clean depended on whether it ran before or after cargo got there. Two
  releases off the same pipeline disagreed, and 0.2.0-rc.1 shipped branding
  itself as built from edited source.

  `built by` already separates a released binary from a hand-built one, which is
  the distinction that carries weight.


## foodstuffs-nz-cli/v0.2.0-rc.1 (2026-08-31)

### Fixes

- cut a candidate to exercise self-update


## foodstuffs-nz-cli/v0.2.0-rc.0 (2026-08-31)

### Features

- follow the release channel the running build is on
  `fsnz update` skipped every prerelease, so a preview build was stranded: it
  could not reach the next preview, and would not move until a stable release
  passed it.

  The channel now follows from the running version and is not stored. A stable
  build still only sees stable releases. A preview build takes a newer stable
  when one exists, and otherwise walks on through the previews, so it rejoins the
  stable channel by itself.

  `--pre-release` takes the newest release of either channel. A version argument
  installs exactly that release, downgrades included, with the leading `v`
  optional; it pins nothing, so the next plain update follows the rules above.
  `--check` mentions a preview without counting it, so it cannot flip the exit
  code a script gates on.


## foodstuffs-nz-cli/v0.1.4-rc.1 (2026-08-31)

### Fixes

- stamp the released version into the binary
  The build matrix runs `dispat run`, which has no version stage, so the
  checked-out Cargo.toml still carries the previous release. Every published
  binary reported it: the 0.1.4-rc.0 tarball holds a binary saying 0.1.3, with
  no release tag. build.rs now prefers DISPAT_NEW_VERSION and DISPAT_TAG,
  falling back to the manifest and `git describe` outside a release.

  The update test mounted its "already newest" release at the running version,
  which a prerelease build filters back out; it now also mounts an older stable
  release, so it holds on either channel.


## foodstuffs-nz-cli/v0.1.4-rc.0 (2026-08-31)

### Fixes

- exercise the dispat release pipeline
