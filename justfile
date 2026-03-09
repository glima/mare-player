name := 'cosmic-applet-mare'
standalone-name := 'mare-player'
appid := 'io.github.cosmic-applet-mare'
features := env('FEATURES', '--all-features')
rootdir := ''
prefix := '/usr'
bloat-target := cargo-target-dir / 'release-bloat' / name

# Installation paths

base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
appdata-dst := base-dir / 'share' / 'appdata' / appid + '.metainfo.xml'
bin-dst := base-dir / 'bin' / name
standalone-bin-dst := base-dir / 'bin' / standalone-name
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

# Compiles with release profile
build-release *args: (build-debug '--release' args)

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
    cargo generate-rpm

# Compiles standalone (no panel applet) with debug profile, renames binary
build-debug-standalone *args:
    cargo build --no-default-features {{ args }}
    cp -f {{ cargo-target-dir / 'debug' / name }} {{ cargo-target-dir / 'debug' / standalone-name }}

# Compiles standalone (no panel applet) with release profile, renames binary
build-release-standalone *args:
    cargo build --release --no-default-features {{ args }}
    cp -f {{ cargo-target-dir / 'release' / name }} {{ cargo-target-dir / 'release' / standalone-name }}

# Compiles standalone release profile with vendored dependencies
build-vendored-standalone *args: vendor-extract
    cargo build --release --no-default-features --frozen --offline {{ args }}
    cp -f {{ cargo-target-dir / 'release' / name }} {{ cargo-target-dir / 'release' / standalone-name }}

# Runs a clippy check, unused import check, and security audit
check *args:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy --all-features {{ args }} -- -W dead_code -D warnings
    echo "Checking for unused imports..."
    if command -v cargo >/dev/null 2>&1 && cargo --list | grep -q machete; then
        cargo machete || exit 1
    else
        echo "cargo-machete not found, skipping unused import check (install with: cargo install cargo-machete)"
    fi
    echo "Running cargo audit for security vulnerabilities..."
    if command -v cargo-audit >/dev/null 2>&1; then
        # All ignored advisories are transitive deps from libcosmic/iced
        # that we cannot fix or upgrade ourselves.
        cargo audit \
            --ignore RUSTSEC-2024-0388 `# derivative (unmaintained) — via zbus 3 → atspi → accesskit → iced` \
            --ignore RUSTSEC-2024-0384 `# instant (unmaintained) — via parking_lot 0.11 → wasm-timer → iced_futures` \
            --ignore RUSTSEC-2024-0436 `# paste (unmaintained) — via metal/accesskit_windows → wgpu/iced` \
            --ignore RUSTSEC-2026-0002 `# lru (unsound) — via iced_glyphon → iced_wgpu → libcosmic`
    else
        echo "cargo-audit not found, skipping security audit (install with: cargo install cargo-audit)"
    fi

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
    just features='--all-features' test {{ args }}
    echo ""
    echo "═══ Testing: standalone (no default features) ═══"
    just features='--no-default-features' test {{ args }}
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
    mkdir -p {{ coverage-dir }}
    echo "Generating HTML coverage report"
    rm -rf {{ coverage-dir }}
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
    install -Dm0644 resources/app.desktop {{ desktop-dst }}
    install -Dm0644 resources/app.metainfo.xml {{ appdata-dst }}
    install -Dm0644 resources/icon.svg {{ icon-dst }}
    install -Dm0644 resources/icon.svg {{ icon-symbolic-dst }}
    install -Dm0644 resources/icon.svg {{ icon-scalable-symbolic-dst }}

# Internal: install standalone binary from the given profile plus shared resources.

# Patches the applet .desktop and metainfo.xml files for standalone mode.
[private]
_install-standalone profile:
    install -Dm0755 {{ cargo-target-dir / profile / standalone-name }} {{ standalone-bin-dst }}
    sed -e 's|^Exec=cosmic-applet-mare|Exec=mare-player|' \
        -e 's|^Comment=.*|Comment=Maré Player — TIDAL streaming for COSMIC|' \
        -e 's|^NoDisplay=true|NoDisplay=false|' \
        -e '/^X-CosmicApplet=/d' \
        -e '/^X-CosmicHoverPopup=/d' \
        resources/app.desktop > {{ desktop-dst }}
    chmod 644 {{ desktop-dst }}
    sed -e 's|<summary>.*</summary>|<summary>Maré Player — TIDAL streaming for COSMIC desktop</summary>|' \
        -e 's|Stream TIDAL from your COSMIC panel\.|Stream TIDAL with|' \
        -e 's|all without leaving|all from a standalone|' \
        -e 's|your desktop\.|COSMIC application.|' \
        -e 's|<binary>cosmic-applet-mare</binary>|<binary>mare-player</binary>|' \
        -e 's|Maré Player Developers|Maré Player Developers|' \
        -e '/<keyword>applet<\/keyword>/d' \
        -e 's|screenshot_applet\.png|screenshot__SWAP.png|' \
        -e 's|screenshot_standalone\.png|screenshot_applet.png|' \
        -e 's|screenshot__SWAP\.png|screenshot_standalone.png|' \
        -e 's|Panel applet with library collection and now-playing bar|__SWAP_CAPTION|' \
        -e 's|Standalone window showing album detail view|Panel applet with library collection and now-playing bar|' \
        -e 's|__SWAP_CAPTION|Standalone window showing album detail view|' \
        resources/app.metainfo.xml > {{ appdata-dst }}
    chmod 644 {{ appdata-dst }}
    install -Dm0644 resources/icon.svg {{ icon-dst }}
    install -Dm0644 resources/icon.svg {{ icon-symbolic-dst }}
    install -Dm0644 resources/icon.svg {{ icon-scalable-symbolic-dst }}

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
    rm -f {{ bin-dst }} {{ standalone-bin-dst }} {{ desktop-dst }} {{ icon-dst }} {{ icon-symbolic-dst }} {{ icon-scalable-symbolic-dst }}

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

# Bump cargo version, create git commit, and create tag (usage: just tag v0.1.0)
tag version:
    #!/usr/bin/env sh
    set -eu
    cargo_version="{{ trim_start_match(version, "v") }}"
    tag="v${cargo_version}"
    find -type f -name Cargo.toml -exec sed -i "0,/^version/s/^version.*/version = \"${cargo_version}\"/" '{}' \; -exec git add '{}' \;
    cargo check
    cargo clean
    git add Cargo.lock
    git commit -m "release: ${tag}"
    git tag -a "${tag}" -m ''
