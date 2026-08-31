# Contributing to Maré Player

Thanks for wanting to help. This document covers what you need installed,
what to run before opening a pull request, and the two rules that are
enforced rather than suggested: [Conventional
Commits](https://www.conventionalcommits.org/en/v1.0.0/) and GPL-3.0-compatible
dependencies.

## Development setup

### Prerequisites

- A recent stable Rust toolchain (edition 2024, `style_edition = "2024"`).
- The [`just`](https://github.com/casey/just) task runner: `cargo install just`.
- System build dependencies, by distro:

  ```sh
  # Fedora / RHEL
  sudo dnf install dbus-devel libsecret-devel libxkbcommon-devel \
      gstreamer1-devel gstreamer1-plugins-base-devel

  # Ubuntu / Debian
  sudo apt install libdbus-1-dev libsecret-1-dev libxkbcommon-dev \
      libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev

  # Arch
  sudo pacman -S dbus libsecret libxkbcommon gstreamer gst-plugins-base
  ```

  Playback also needs the GStreamer runtime plugin set (base, good, bad and
  libav) — see [Dependencies](./README.md#dependencies) in the README. If
  your distro needs something not listed, a PR updating both files is
  welcome.

- Optional, but what CI uses:
  `cargo install cargo-nextest cargo-machete cargo-audit cargo-llvm-cov \
  cargo-deny cocogitto`.
  The recipes fall back or skip when a tool is missing, so you can start
  without them.

### Build and run

```sh
just run                 # panel applet (default features)
just run-standalone      # standalone window (--no-default-features)
just build-release       # optimized build
just install             # install to /usr (needs sudo)
```

Both modes are built from the same sources; `panel-applet` is a default
feature and the standalone build turns it off. A change that touches the UI
should be tried in both — `just test-matrix` compiles and tests both, but it
won't tell you that a popup looks wrong.

## Layout

| Path                  | Role                                                          |
|-----------------------|---------------------------------------------------------------|
| `src/tidal/`          | TIDAL client, PKCE auth, models, MPRIS, play reporting        |
| `src/playback/`       | GStreamer pipelines for audio and video, gapless, replay gain |
| `src/handlers/`       | Message handlers — one module per concern                     |
| `src/views/`          | Everything rendered, applet popup and standalone alike        |
| `src/cache/`          | Embedded database behind the view and image caches            |
| `src/audio/`          | FFT spectrum analysis for the visualizer                      |
| `mare-video-window/`  | Out-of-process video window (a separate workspace member)      |
| `i18n/`               | Fluent translations, one directory per locale                 |
| `tests/`              | Integration tests                                             |
| `fuzz/`               | cargo-fuzz targets (its own workspace)                        |

## Before opening a pull request

```sh
just check          # clippy -D warnings, unused deps, security audit, i18n
just test-matrix    # tests for both feature sets
cargo fmt --all     # or `just fmt`
```

`just check` is what CI gates on, and it is stricter than plain clippy:

- **Production lints are denied**, not warned: `unwrap_used`, `expect_used`,
  `panic`, `indexing_slicing`, `wildcard_imports` (see `[lints.clippy]` in
  `Cargo.toml`). Test code may relax them with a crate-level `#![allow]`;
  `src/` should not. If you need one, the exception belongs in a comment
  saying why it is safe.
- **`cargo machete`** fails on dependencies nothing imports.
- **`cog check`** fails on a commit message that isn't a Conventional
  Commit — see below.
- **`cargo deny check licenses`** fails on a dependency whose licence
  isn't GPL-3.0-compatible — see below.
- **`cargo audit`** fails on advisories. If an advisory is unreachable or
  unfixable from here, it goes in the ignore list in the `justfile` with the
  dependency path, why it can't bite, and what clears it — not just an ID.
- **`just i18n-check`** fails when a locale is missing keys that `i18n/en/`
  has. Adding a UI string means adding it to all locales; leaving one out
  breaks the build rather than falling back silently.

Also useful:

```sh
just doc            # rustdoc with private items; should be warning-free
just coverage       # llvm-cov summary
just bloat-check    # where binary size goes
just test-verbose   # test output as it runs
```

## Commit messages

**[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) are
required.** The history is uniform and stays that way:

```
type(scope): imperative summary, lowercase, no trailing period

Body explaining what was wrong and why this is the fix, wrapped at 72
columns. Findings and measurements belong here — they are the part that
survives.

BREAKING CHANGE: only when a user has to do something differently.
```

- **Types**: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`,
  `ci`, `chore`, `style`, plus `security`, `deps`, `packaging`, `license` and
  `release`. That list is [`cog.toml`](./cog.toml), and it is checked: `just
  check` runs `cog check --from-latest-tag`, so an unknown type or a
  non-conventional subject fails the build. Each type also has a section in
  [`cliff.toml`](./cliff.toml), so anything that passes the check appears in
  the release notes. A `!` before the colon marks a breaking change, pairs
  with a `BREAKING CHANGE:` trailer, and earns its own section.
- **Scopes** are the area touched, not the file: `auth`, `playback`,
  `quality`, `cache`, `ui`, `nav`, `lyrics`, `video`, `deps`, `audit`, `i18n`.
- **Bodies matter more than summaries.** Say what the behaviour was, what it
  is now, and how you know — an error message, a measured number, an API
  response. "Fix bug" tells the next reader nothing; the commit is where the
  evidence lives.
- **One concern per commit.** Formatting churn goes in its own `style:`
  commit so it doesn't bury a fix.

## Dependencies and licence compatibility

Maré Player is **GPL-3.0-only**, so every dependency has to be
GPL-3.0-compatible. `just check` enforces this with `cargo deny`, against
the allow list in [`deny.toml`](./deny.toml): permissive licences (MIT,
Apache-2.0, BSD, ISC, Zlib, Unicode-3.0, 0BSD, CC0, BSL-1.0) plus MPL-2.0
and GPL-3.0 itself.

Two things that catch people out:

- **Transitive licences count.** A permissive crate that pulls a copyleft
  one brings it into the binary all the same, which is how this project
  ended up GPL-3.0 in the first place — libcosmic's `wayland` feature
  enables `cctk`, and `cosmic-protocols` is `GPL-3.0-only`.
- **GPL-incompatible is not the same as non-free.** CDDL and
  Apache-2.0-only-under-GPLv2 are perfectly good licences that still can't
  be linked here. If you need one, it goes in `deny.toml` as a scoped
  exception with a comment saying why it is safe — `inferno` is there
  because the profiler that pulls it is compiled only in debug builds.

Check before proposing:

```sh
cargo deny check licenses          # the gate `just check` runs
cargo tree -e normal -p <crate>    # what it drags in
```

GStreamer is used through its **Rust bindings (MIT)** with the plugins
loaded at runtime from the system, so nothing LGPL is linked into the
binary. That separation is deliberate; keep it.

New dependencies also need a reason in the commit body: what it does, why
the standard library or an existing dependency doesn't, and what it costs
in binary size (`just bloat-check`).

## Translations

Locales live in `i18n/<lang>/cosmic_applet_mare.ftl`, with `i18n/en/` as the
reference. Keep keys in the same order as `en`, translate the text rather
than the key names, and run `just i18n-check` before submitting. Strings with
units, TIDAL's own tier names, or anything shown next to data from the API
are deliberately not localized — see `AudioQuality::display_name`.

## Pull requests

1. Make sure `just check` and `just test-matrix` pass locally. CI runs the
   same recipes, so a green local run usually means a green CI run.
2. Describe what changed and why, and reference the issue if there is one.
3. Add tests for behaviour that can be tested without a TIDAL account —
   parsing, models, helpers, state transitions. Anything that needs the API
   is exercised by hand; say what you tried in the PR.
4. Keep the branch rebased on `main`. History is linear; merges are rebase
   merges.

## Reporting bugs

Include your distro and desktop, whether you're on the applet or standalone
build, and the log:

```sh
journalctl --user -o cat | grep cosmic-applet-mare      # applet, via cosmic-panel
mare-player 2>&1 | tee /tmp/mare.log                    # standalone
```

Settings has a log-level control if you need `debug` or `trace`. Redact your
access token if you paste raw log lines — it appears in some requests.

## Security

Report anything involving credentials, the keyring, or the OAuth flow
privately through GitHub's [security
advisories](https://github.com/glima/mare-player/security/advisories/new)
rather than a public issue.

Never commit a token, a session dump, or a `client_secret`. The OAuth client
we authenticate as is documented in `src/tidal/client_identity.rs`; its
secret stays in the `tidlers` crate and must not be copied into this
repository.

## License

By contributing, you agree that your contributions are licensed under the
[GNU General Public License v3.0](./LICENSE).
