#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
#
# Derive the standalone application's desktop entry and AppStream metadata
# from the panel applet's (resources/app.desktop, resources/app.metainfo.xml).
#
# Usage: sh packaging/standalone-resources.sh RESOURCES_DIR OUT_DIR
#
# Writes OUT_DIR/app.desktop and OUT_DIR/app.metainfo.xml; OUT_DIR may be
# RESOURCES_DIR. This is the one place the standalone differences live: the
# just install recipe, the release workflow (.deb, .rpm, tarball) and the
# COPR spec all call it. The spec ships it as a separate source, so packaging
# an older tag still applies the current rules.
set -eu

if [ $# -ne 2 ]; then
    echo "usage: $0 RESOURCES_DIR OUT_DIR" >&2
    exit 2
fi
src=$1
out=$2
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Keep the shared application ID and the tidal:// handler, but make the
# launcher visible, point it at the standalone binary, and stop declaring a
# panel applet.
sed -e 's|^Exec=cosmic-applet-mare|Exec=mare-player|' \
    -e 's|^Comment=.*|Comment=Maré Player — TIDAL streaming for COSMIC|' \
    -e 's|^NoDisplay=true|NoDisplay=false|' \
    -e '/^X-CosmicApplet=/d' \
    -e '/^X-CosmicHoverPopup=/d' \
    "$src/app.desktop" > "$tmp/app.desktop"

# Describe the window application, name its binary, and lead with the
# standalone screenshot.
sed -e 's|<summary>.*</summary>|<summary>TIDAL streaming for the COSMIC desktop</summary>|' \
    -e 's|Stream TIDAL from your COSMIC panel\.|Stream TIDAL with|' \
    -e 's|all without leaving|all from a standalone|' \
    -e 's|your desktop\.|COSMIC application.|' \
    -e 's|<binary>cosmic-applet-mare</binary>|<binary>mare-player</binary>|' \
    -e '/<keyword>applet<\/keyword>/d' \
    -e 's|screenshot_applet\.png|screenshot__SWAP.png|' \
    -e 's|screenshot_standalone\.png|screenshot_applet.png|' \
    -e 's|screenshot__SWAP\.png|screenshot_standalone.png|' \
    -e 's|Panel applet with library collection and now-playing bar|__SWAP_CAPTION|' \
    -e 's|Standalone window with library collection and a playing music video|Panel applet with library collection and now-playing bar|' \
    -e 's|__SWAP_CAPTION|Standalone window with library collection and a playing music video|' \
    "$src/app.metainfo.xml" > "$tmp/app.metainfo.xml"

# If the applet files change shape and a rewrite stops matching, fail rather
# than ship applet metadata with the standalone app.
check() {
    if ! grep -q "$1" "$tmp/$2"; then
        echo "$0: $2: expected '$1' after rewriting; update this script" >&2
        exit 1
    fi
}
check '^Exec=mare-player ' app.desktop
check '^NoDisplay=false$' app.desktop
check '<binary>mare-player</binary>' app.metainfo.xml
if grep -q '^X-CosmicApplet=\|cosmic-applet-mare %u' "$tmp/app.desktop"; then
    echo "$0: app.desktop still describes the panel applet" >&2
    exit 1
fi

mkdir -p "$out"
cat "$tmp/app.desktop" > "$out/app.desktop"
cat "$tmp/app.metainfo.xml" > "$out/app.metainfo.xml"
