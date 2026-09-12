#!/usr/bin/env bash
# deploy.sh — build release, install to ~/.local/bin, update desktop entry.
# Run after any code change so the docked app is always current.
set -euo pipefail
cd "$(dirname "$0")"

echo "==> building release binary (with embedded frontend)…"
# Touch main.rs to force cargo to recompile — cargo skips rebuilds when only
# frontend dist changes, but the dist is embedded at compile time.
touch src-tauri/src/main.rs
cargo tauri build --no-bundle 2>&1 | tail -5

INSTALL_DIR="$HOME/.local/bin"
DESKTOP_DIR="$HOME/.local/share/applications"

# Install binary
mkdir -p "$INSTALL_DIR"
cp target/release/cyberdeck "$INSTALL_DIR/cyberdeck.tmp"
mv -f "$INSTALL_DIR/cyberdeck.tmp" "$INSTALL_DIR/cyberdeck"
chmod +x "$INSTALL_DIR/cyberdeck"
# Keep cyberdeck-stable in sync (the docked launcher)
cp "$INSTALL_DIR/cyberdeck" "$INSTALL_DIR/cyberdeck-stable"

# Install icon
mkdir -p "$HOME/.local/share/icons"
cp src-tauri/icons/icon.png "$HOME/.local/share/icons/cyberdeck.png"

# Install .desktop file
mkdir -p "$DESKTOP_DIR"
cat > "$DESKTOP_DIR/cyberdeck.desktop" <<EOF
[Desktop Entry]
Name=Cyberdeck
Comment=Local LLM fleet manager
Exec=env GDK_BACKEND=x11 $INSTALL_DIR/cyberdeck
Icon=$HOME/.local/share/icons/cyberdeck.png
Terminal=false
Type=Application
Categories=Development;Utility;
StartupWMClass=cyberdeck
EOF

echo "==> deployed: $INSTALL_DIR/cyberdeck ($(stat -c%s "$INSTALL_DIR/cyberdeck") bytes)"
