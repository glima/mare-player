name := 'cosmic-applet-mare'
standalone-name := 'mare-player'
video-window-name := 'mare-video-window'
appid := 'io.github.cosmic-applet-mare'
features := env('FEATURES', '--all-features')
rootdir := ''
prefix := '/usr'
bloat-target := cargo-target-dir / 'release-bloat' / name

# `install` normally runs under sudo, but the `tidal://` scheme association is
# per-user config: run that one command back as the invoking user, or it lands
# in root's home and the sign-in silently falls back to copying a URL.
as-invoking-user := if env('SUDO_USER', '') != '' { 'sudo -u ' + env('SUDO_USER', '') } else { '' }

# Installation paths

base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
appdata-dst := base-dir / 'share' / 'appdata' / appid + '.metainfo.xml'
bin-dst := base-dir / 'bin' / name
standalone-bin-dst := base-dir / 'bin' / standalone-name
video-window-bin-dst := base-dir / 'bin' / video-window-name
desktop-dst := base-dir / 'share' / 'applications' / appid + '.desktop'
icon-dst := base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg'
icon-symbolic-dst := base-dir / 'share' / 'icons' / 'hicolor' / 'symbolic' / 'apps' / appid + '-symbolic.svg'
icon-scalable-symbolic-dst := base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '-symbolic.svg'

default: build-release

clean:
    rm -rf {{ coverage-dir }}
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build {{ args }}

# Compiles with release profile (applet + the mare-video-window companion that
# renders popped-out videos out of process)
build-release *args: (build-debug '--release' args)
    cargo build --release -p {{ video-window-name }} {{ args }}

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Compiles and packages a .deb (requires cargo-deb)
build-deb: build-release
    command -v cargo-deb || cargo install cargo-deb
    cargo deb --no-build

# Compiles and packages an .rpm (requires cargo-generate-rpm)
build-rpm: build-release
    command -v cargo-generate-rpm || cargo install cargo-generate-rpm
    strip -s {{ cargo-target-dir / 'release' / name }}
    strip -s {{ cargo-target-dir / 'release' / video-window-name }}
    cargo generate-rpm

# Prepares both vendored source RPMs for COPR (does not compile or upload)
build-srpm ref='HEAD' release='1':
    bash packaging/rpm/build-srpm.sh {{ quote(ref) }} {{ quote(release) }}

# Compiles standalone (no panel applet) with debug profile, renames binary
# The standalone top-level window can use the GPU, so it keeps the `wgpu`
# feature (the applet build omits it and renders on tiny_skia — see Cargo.toml).
build-debug-standalone *args:
    cargo build --no-default-features --features wgpu {{ args }}
    cp -f {{ cargo-target-dir / 'debug' / name }} {{ cargo-target-dir / 'debug' / standalone-name }}

# Compiles standalone (no panel applet) with release profile, renames binary
build-release-standalone *args:
    cargo build --release --no-default-features --features wgpu {{ args }}
    cp -f {{ cargo-target-dir / 'release' / name }} {{ cargo-target-dir / 'release' / standalone-name }}

# Compiles standalone release profile with vendored dependencies
build-vendored-standalone *args: vendor-extract
    cargo build --release --no-default-features --features wgpu --frozen --offline {{ args }}
    cp -f {{ cargo-target-dir / 'release' / name }} {{ cargo-target-dir / 'release' / standalone-name }}

# Runs a formatting check, clippy check, unused import check, and security audit
check *args:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Checking formatting..."
    cargo fmt --all -- --check
    cargo clippy --all-features {{ args }} -- -W dead_code -D warnings
    # Clippy above deliberately omits --all-targets: the production lint set
    # (unwrap_used, indexing_slicing, …) is wrong for test code. That leaves
    # integration tests uncompiled, so build them here — a signature change in
    # the library can otherwise break `tests/` without this recipe noticing.
    echo "Checking that tests compile..."
    cargo test --no-run --all-features
    echo "Checking for unused imports..."
    if command -v cargo >/dev/null 2>&1 && cargo --list | grep -q machete; then
        cargo machete || exit 1
    else
        echo "cargo-machete not found, skipping unused import check (install with: cargo install cargo-machete)"
    fi
    echo "Checking commit messages..."
    if command -v cog >/dev/null 2>&1; then
        # Conventional Commits, from the last tag forward — the history before
        # the convention stays as it is. Types beyond the defaults are declared
        # in cog.toml, and each one has a section in cliff.toml, so a subject
        # that passes here cannot fall out of the release notes.
        cog check --from-latest-tag || exit 1
    else
        echo "cocogitto not found, skipping commit-message check (install with: cargo install cocogitto)"
    fi
    echo "Checking dependency licences..."
    if command -v cargo-deny >/dev/null 2>&1; then
        # Allow list and exceptions live in deny.toml. The project is
        # GPL-3.0-only, so this gates what a new dependency may drag in —
        # the licence of a transitive crate is as binding as a direct one.
        cargo deny check licenses --hide-inclusion-graph || exit 1
    else
        echo "cargo-deny not found, skipping licence check (install with: cargo install cargo-deny)"
    fi
    echo "Running cargo audit for security vulnerabilities..."
    if command -v cargo-audit >/dev/null 2>&1; then
        # All ignored advisories are transitive deps from libcosmic/iced
        # that we cannot fix or upgrade ourselves.
        cargo audit \
            --ignore RUSTSEC-2024-0436 `# paste (unmaintained) — via metal (macOS-only) → wgpu-hal → wgpu → cryoglyph → iced_wgpu → libcosmic` \
            --ignore RUSTSEC-2026-0186 `# memmap2 0.8 (unsound) — via xkbcommon 0.7 → iced_winit → libcosmic. Everything else here is already on the unaffected 0.9.11; clears when iced_winit moves to xkbcommon 0.9, which asks for memmap2 ^0.9` \
            --ignore RUSTSEC-2026-0192 `# ttf-parser (unmaintained) — via fontdb → cosmic-text, owned_ttf_parser → ab_glyph → accesskit_winit, and rustybuzz → resvg; all three land in libcosmic` \
            --ignore RUSTSEC-2026-0206 `# rustybuzz (unmaintained) — via resvg/usvg → iced_tiny_skia → iced → libcosmic (SVG/text shaping)` \
            --ignore RUSTSEC-2026-0194 `# quick-xml DoS — via pprof→inferno 0.11→quick-xml 0.26; the SIGUSR1 flamegraph profiler is debug-builds-only and parses its own output, never untrusted input. pprof 0.15 (latest) pins inferno ^0.11, so the fixed quick-xml >=0.41 is out of reach until pprof moves to inferno 0.12` \
            --ignore RUSTSEC-2026-0195 `# quick-xml DoS — same pprof→inferno→quick-xml 0.26 path. (The old wayland-scanner path is fixed: 0.31.11 moved to quick-xml 0.41.)` \
            --ignore RUSTSEC-2026-0253 `# lru (unsound) — via cryoglyph → iced → libcosmic, which requires lru ^0.16 while the fix landed in 0.18.2, so cargo cannot reach it. The unsoundness needs a key whose Drop panics under catch_unwind; cryoglyph's single cache is keyed by cosmic-text's CacheKey, a Copy struct of integers with no Drop at all. Clears when cryoglyph moves to lru 0.18`
    else
        echo "cargo-audit not found, skipping security audit (install with: cargo install cargo-audit)"
    fi
    echo "Checking i18n locale completeness..."
    just i18n-check

# Reformat the whole workspace (fixes what `just check`'s rustfmt gate reports)
fmt:
    cargo fmt --all

# Run tests (override features via: just features='--no-default-features' test)
test *args:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Testing with: {{ features }}"
    if command -v cargo-nextest >/dev/null 2>&1; then
        cargo nextest run {{ features }} --no-fail-fast --status-level=skip {{ args }}
    else
        echo "cargo-nextest not found, falling back to cargo test"
        cargo test {{ features }} {{ args }}
    fi

# Run tests with verbose output
test-verbose *args:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Testing with: {{ features }}"
    if command -v cargo-nextest >/dev/null 2>&1; then
        NEXTEST_SHOW_OUTPUT=always cargo nextest run {{ features }} \
            --no-fail-fast -v --status-level=skip \
            --success-output=immediate --failure-output=immediate {{ args }}
    else
        echo "cargo-nextest not found, falling back to cargo test"
        cargo test {{ features }} -- --nocapture {{ args }}
    fi

# Run tests for both applet and standalone feature sets
test-matrix *args:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "═══ Testing: panel-applet (default features) ═══"
    just features='--all-features' test --profile applet {{ args }}
    echo ""
    echo "═══ Testing: standalone (no default features) ═══"
    just features='--no-default-features --features wgpu' test --profile standalone {{ args }}
    echo ""
    echo "All feature combinations passed ✓"

# Coverage directory

coverage-dir := 'coverage'

# Run coverage analysis (HTML + LCOV) with cargo-llvm-cov
coverage:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
        echo "Error: cargo-llvm-cov not found. Install with: cargo install cargo-llvm-cov"
        exit 1
    fi
    THREADS=$(( $(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4) / 2 ))
    [ "$THREADS" -lt 1 ] && THREADS=1
    rm -rf {{ coverage-dir }}
    mkdir -p {{ coverage-dir }}
    echo "Generating HTML coverage report"
    cargo llvm-cov --all-features \
        --html \
        --ignore-filename-regex '/tests?/|/target/' \
        -- --test-threads="$THREADS"
    if [ -d target/llvm-cov/html ]; then
        cp -r target/llvm-cov/html {{ coverage-dir }}/html
    fi
    echo "Generating LCOV report"
    cargo llvm-cov --all-features \
        --no-clean \
        --lcov --output-path {{ coverage-dir }}/lcov.info \
        --ignore-filename-regex '/tests?/|/target/' \
        --summary-only \
        -- --test-threads="$THREADS"
    echo ""
    echo "Coverage reports:"
    echo "  HTML: {{ coverage-dir }}/html/index.html"
    echo "  LCOV: {{ coverage-dir }}/lcov.info"
    if command -v xdg-open >/dev/null 2>&1; then
        xdg-open {{ coverage-dir }}/html/index.html >/dev/null 2>&1 || true
    fi

# Print a text-only coverage summary (no HTML output)
coverage-summary:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
        echo "Error: cargo-llvm-cov not found. Install with: cargo install cargo-llvm-cov"
        exit 1
    fi
    THREADS=$(( $(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4) / 2 ))
    [ "$THREADS" -lt 1 ] && THREADS=1
    cargo llvm-cov --all-features --no-report \
        -- --test-threads="$THREADS"
    cargo llvm-cov report --summary-only

# Build documentation (including private items)
doc *args:
    cargo doc --document-private-items --no-deps {{ args }}

# Build documentation and open in browser
doc-open: (doc '--open')

# Assemble the project website (site/ plus artwork from resources/) into
# target/site, exactly as the Pages workflow publishes it. The download
# table is filled from the latest GitHub release when `gh` can reach it,
# and falls back to a link to the release page otherwise. Preview with
# `python3 -m http.server --directory target/site 8000`.
site out=(cargo-target-dir / 'site'):
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf "{{ out }}"
    mkdir -p "{{ out }}/assets"
    cp site/index.html site/style.css "{{ out }}/"
    cp resources/icon.svg resources/screenshot_applet.png resources/screenshot_standalone.png "{{ out }}/assets/"
    { gh release view --repo glima/mare-player --json tagName,url,assets 2>/dev/null || true; } \
        | python3 site/downloads.py "{{ out }}/index.html"
    echo "Site assembled in {{ out }}"

# Run the application for testing purposes
run *args:
    env RUST_BACKTRACE=full cargo run --release {{ args }}

# Run standalone (no panel applet) for testing purposes
run-standalone *args: build-release-standalone
    env RUST_BACKTRACE=full {{ cargo-target-dir / 'release' / standalone-name }} {{ args }}

# Run the application for testing purposes
run-debug *args:
    env RUST_BACKTRACE=full cargo run {{ args }}

# Run standalone (no panel applet) for testing purposes
run-standalone-debug *args: build-debug-standalone
    env RUST_BACKTRACE=full {{ cargo-target-dir / 'debug' / standalone-name }} {{ args }}

# Internal: install binary from the given profile plus shared resources
[private]
_install profile:
    install -Dm0755 {{ cargo-target-dir / profile / name }} {{ bin-dst }}
    install -Dm0755 {{ cargo-target-dir / profile / video-window-name }} {{ video-window-bin-dst }}
    install -Dm0644 resources/app.desktop {{ desktop-dst }}
    install -Dm0644 resources/app.metainfo.xml {{ appdata-dst }}
    install -Dm0644 resources/icon.svg {{ icon-dst }}
    install -Dm0644 resources/icon.svg {{ icon-symbolic-dst }}
    install -Dm0644 resources/icon.svg {{ icon-scalable-symbolic-dst }}

    # Route `tidal://login/auth?code=…` to the player, so the TIDAL sign-in
    # returns by itself instead of asking for the URL to be copied.
    #
    # The association is per-user while this recipe usually runs under sudo, so
    # it has to be set as the invoking user — as root it lands in /root and the
    # sign-in silently falls back to the paste box. Best-effort either way.
    -update-desktop-database {{ base-dir }}/share/applications 2>/dev/null
    -{{ as-invoking-user }} xdg-mime default io.github.cosmic-applet-mare.desktop x-scheme-handler/tidal

# Internal: install standalone binary from the given profile plus shared resources.

# The desktop entry and metainfo come from packaging/standalone-resources.sh,
# the same rule the release packages and the COPR spec use.
[private]
_install-standalone profile:
    install -Dm0755 {{ cargo-target-dir / profile / standalone-name }} {{ standalone-bin-dst }}
    # A temporary directory, not target/: this usually runs under sudo.
    d=$(mktemp -d) && sh packaging/standalone-resources.sh resources "$d" \
        && install -Dm0644 "$d/app.desktop" {{ desktop-dst }} \
        && install -Dm0644 "$d/app.metainfo.xml" {{ appdata-dst }}; s=$?; rm -rf "$d"; exit $s
    install -Dm0644 resources/icon.svg {{ icon-dst }}
    install -Dm0644 resources/icon.svg {{ icon-symbolic-dst }}
    install -Dm0644 resources/icon.svg {{ icon-scalable-symbolic-dst }}

    # Same `tidal://` handler registration as the applet install — see there.
    -update-desktop-database {{ base-dir }}/share/applications 2>/dev/null
    -{{ as-invoking-user }} xdg-mime default io.github.cosmic-applet-mare.desktop x-scheme-handler/tidal

# Installs release build
install: (_install 'release')

# Installs debug build (unstripped, unoptimised — useful for debugging)
install-debug: (_install 'debug')

# Installs standalone release build
install-standalone: (_install-standalone 'release')

# Installs standalone debug build
install-standalone-debug: (_install-standalone 'debug')

# Uninstalls installed files
uninstall:
    rm -f {{ bin-dst }} {{ video-window-bin-dst }} {{ standalone-bin-dst }} {{ desktop-dst }} {{ icon-dst }} {{ icon-symbolic-dst }} {{ icon-scalable-symbolic-dst }}

# Check that all locale .ftl files have the same keys as the English reference
i18n-check:
    #!/usr/bin/env bash
    set -euo pipefail
    ref="i18n/en/cosmic_applet_mare.ftl"
    en_keys=$(grep -oP '^[a-z][-a-z0-9]*' "$ref" | sort)
    en_count=$(echo "$en_keys" | wc -l)
    echo "Reference: en ($en_count keys)"
    echo ""
    fail=0
    for dir in i18n/*/; do
        lang=$(basename "$dir")
        [ "$lang" = "en" ] && continue
        ftl="$dir/cosmic_applet_mare.ftl"
        lang_keys=$(grep -oP '^[a-z][-a-z0-9]*' "$ftl" | sort)
        missing=$(comm -23 <(echo "$en_keys") <(echo "$lang_keys"))
        extra=$(comm -13 <(echo "$en_keys") <(echo "$lang_keys"))
        m=$(echo "$missing" | grep -c . || true)
        e=$(echo "$extra"   | grep -c . || true)
        if [ "$m" -gt 0 ] || [ "$e" -gt 0 ]; then
            echo "$lang: missing=$m extra=$e"
            [ "$m" -gt 0 ] && echo "  missing: $missing" | tr '\n' ' ' && echo
            [ "$e" -gt 0 ] && echo "  extra:   $extra"   | tr '\n' ' ' && echo
            fail=1
        fi
    done
    if [ "$fail" -eq 0 ]; then
        echo "All locales up to date ✓"
    else
        echo ""
        echo "Some locales are out of sync ✗"
        exit 1
    fi

# Vendor dependencies locally
vendor:
    mkdir -p .cargo
    cargo vendor --sync Cargo.toml | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    echo >> .cargo/config.toml
    rm -rf .cargo vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# Analyze binary size by crate and function using cargo-bloat
bloat-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-bloat >/dev/null 2>&1; then
        echo "Error: cargo-bloat not found. Install with: cargo install cargo-bloat"
        exit 1
    fi
    echo ""
    echo "Building with release-bloat profile (preserves symbols)"
    cargo build --profile release-bloat
    echo ""
    echo "Binary Size Overview"
    echo "Analysis binary size (with symbols):"
    ls -lh {{ bloat-target }} | awk '{print "  " $5}'
    echo "Stripped size (production equivalent):"
    TEMP_STRIPPED=$(mktemp)
    cp {{ bloat-target }} "$TEMP_STRIPPED"
    strip "$TEMP_STRIPPED"
    ls -lh "$TEMP_STRIPPED" | awk '{print "  " $5}'
    rm "$TEMP_STRIPPED"
    echo ""
    echo "Section breakdown:"
    size {{ bloat-target }}
    echo ""
    echo "Top 30 Crates by Size"
    cargo bloat --profile release-bloat --crates -n 30
    echo ""
    echo "Top 20 Functions by Size"
    cargo bloat --profile release-bloat -n 20
    echo ""
    echo "Tip: Run 'just build-release' to create production binary with symbols stripped"

# Code statistics via tokei
stats:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v tokei >/dev/null 2>&1; then
        echo "Error: tokei not found. Install with: cargo install tokei"
        exit 1
    fi
    tokei .

# Fuzz-test parsing targets (requires nightly: rustup toolchain install nightly)
# Usage: just fuzz              — run all targets for 60s each
#        just fuzz dash_parse   — run a single target

# just fuzz dash_parse 0 — run until interrupted (Ctrl-C)
fuzz target="" duration="60":
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-fuzz >/dev/null 2>&1; then
        echo "Error: cargo-fuzz not found. Install with: cargo install cargo-fuzz"
        exit 1
    fi
    if [ -n "{{ target }}" ]; then
        echo "═══ Fuzzing: {{ target }} ═══"
        if [ "{{ duration }}" = "0" ]; then
            cargo +nightly fuzz run "fuzz_{{ target }}"
        else
            cargo +nightly fuzz run "fuzz_{{ target }}" -- -max_total_time="{{ duration }}"
        fi
    else
        for t in $(cargo +nightly fuzz list 2>/dev/null); do
            echo ""
            echo "═══ Fuzzing: ${t} ({{ duration }}s) ═══"
            cargo +nightly fuzz run "$t" -- -max_total_time="{{ duration }}"
        done
        echo ""
        echo "All fuzz targets passed ✓"
    fi

# Update Nostr mirrors from origin/main and tags (requires nak and local nip34.json)
mirror-nostr:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -z "${NOSTR_SECRET_KEY:-}" ]]; then
        # Print the command literally; bunkeruri belongs to the caller's shell.
        # shellcheck disable=SC2016
        echo 'Missing bunker URL. Run: NOSTR_SECRET_KEY="$(bunkeruri)" just mirror-nostr' >&2
        exit 1
    fi
    if [[ "$NOSTR_SECRET_KEY" != bunker://* ]]; then
        echo 'NOSTR_SECRET_KEY must be a bunker:// URL from bunkeruri.' >&2
        exit 1
    fi
    if ! command -v nak >/dev/null 2>&1; then
        echo 'Missing nak; install the Nostr CLI before mirroring.' >&2
        exit 1
    fi
    if [[ ! -f nip34.json ]]; then
        echo 'Missing local nip34.json; configure this checkout with nak git init first.' >&2
        exit 1
    fi
    git fetch origin --tags
    nak git push --tags --no-announcement origin/main:main

# Bump cargo version, create git commit, and create tag (usage: just tag v0.1.0 "Ocean Breeze")
tag version name="":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo_version="{{ trim_start_match(version, "v") }}"
    tag="v${cargo_version}"
    # Every tracked manifest, from git rather than a filesystem search, which
    # would also reach untracked copies (e.g. Delta worktrees under .delta/).
    mapfile -t manifests < <(git ls-files ':(glob)**/Cargo.toml')
    # Only the [package] version: the first `version` line could belong to a
    # dependency table.
    for manifest in "${manifests[@]}"; do
        sed -i "/^\[package\]/,/^\[/s/^version = \".*\"/version = \"${cargo_version}\"/" "$manifest"
    done
    for manifest in "${manifests[@]}"; do
        if ! sed -n '/^\[package\]/,/^\[/p' "$manifest" | grep -qx "version = \"${cargo_version}\""; then
            echo "error: ${manifest} has no [package] version ${cargo_version}" >&2
            exit 1
        fi
    done
    # Each workspace has its own lockfile; fuzz/ is a separate workspace.
    mapfile -t lockfiles < <(git ls-files ':(glob)**/Cargo.lock')
    for lockfile in "${lockfiles[@]}"; do
        cargo update --workspace --manifest-path "$(dirname "$lockfile")/Cargo.toml"
    done
    cargo check
    cargo clean
    git add "${manifests[@]}" "${lockfiles[@]}"
    git commit -m "release: ${tag}"
    git tag -a "${tag}" -m '{{ name }}'

# Cut a release: bump version, tag, and push (usage: just release v0.1.7 "Ocean Breeze")
release version name="":
    #!/usr/bin/env bash
    set -euo pipefail
    tag="v{{ trim_start_match(version, "v") }}"
    if [ -n "{{ name }}" ]; then
        echo "🌊 Release: ${tag} — {{ name }}"
    else
        echo "🌊 Release: ${tag}"
    fi
    echo ""
    read -rp "Proceed? [y/N] " confirm
    [[ "$confirm" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 1; }
    just tag "{{ version }}" '{{ name }}'
    git push origin main
    git push origin "${tag}"
