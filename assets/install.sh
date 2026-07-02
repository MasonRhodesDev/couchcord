#!/usr/bin/env bash
# couchcord installer — one command from clone to running daemon.
#
# Targets SteamOS / Steam Deck (immutable rootfs, no host compiler) but works
# on any systemd Linux. Idempotent: re-run after `git pull` to upgrade.
#
#   git clone https://github.com/MasonRhodesDev/couchcord.git
#   couchcord/assets/install.sh
#
# What it does:
#   1. builds couchcordd (host cargo if present, else a distrobox container)
#   2. installs the binary + the multi-tenant Discord launcher to ~/.local/bin
#   3. installs default config (~/.config/couchcord/config.toml) if missing
#   4. installs + enables the systemd user unit (starts with any graphical
#      session, i.e. when the device launches into a user)
#   5. runs `couchcordd doctor`
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILDER_NAME="couchcord-builder"
BUILDER_IMAGE="docker.io/library/debian:12"   # glibc 2.36 ≤ any 2024+ host

say() { printf '\033[1m[couchcord install]\033[0m %s\n' "$*"; }

# --- 1. build -----------------------------------------------------------------
if command -v cargo >/dev/null 2>&1; then
    say "building with host cargo"
    (cd "$REPO" && cargo build --release)
elif command -v distrobox >/dev/null 2>&1; then
    say "no host toolchain — building in distrobox ($BUILDER_IMAGE)"
    if ! distrobox list 2>/dev/null | grep -q "$BUILDER_NAME"; then
        distrobox create --image "$BUILDER_IMAGE" --name "$BUILDER_NAME" --yes
    fi
    distrobox enter "$BUILDER_NAME" -- bash -lc '
        set -e
        if ! command -v gcc >/dev/null;  then sudo apt-get update -qq && sudo apt-get install -y -qq build-essential curl; fi
        if [ ! -x "$HOME/.cargo/bin/cargo" ]; then curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable; fi
        cd '"$REPO"' && "$HOME/.cargo/bin/cargo" build --release
    '
else
    echo "error: need either cargo or distrobox (SteamOS ships distrobox)" >&2
    exit 1
fi

# --- 2. binaries ---------------------------------------------------------------
install -Dm755 "$REPO/target/release/couchcordd" "$HOME/.local/bin/couchcordd"
install -Dm755 "$REPO/assets/game-mode-discord"  "$HOME/.local/bin/game-mode-discord"
say "installed couchcordd + game-mode-discord to ~/.local/bin"

# --- 3. config -----------------------------------------------------------------
CONF="${XDG_CONFIG_HOME:-$HOME/.config}/couchcord/config.toml"
if [ ! -f "$CONF" ]; then
    install -Dm644 "$REPO/config.toml.example" "$CONF"
    say "installed default config at $CONF"
else
    say "keeping existing config at $CONF"
fi

# --- 4. service ----------------------------------------------------------------
install -Dm644 "$REPO/assets/systemd/couchcordd.service" \
    "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/couchcordd.service"
systemctl --user daemon-reload
systemctl --user enable couchcordd
if systemctl --user is-active graphical-session.target >/dev/null 2>&1; then
    systemctl --user restart couchcordd
    say "service enabled and (re)started"
else
    say "service enabled — starts with the next graphical session"
fi

# --- 5. doctor -----------------------------------------------------------------
say "running doctor"
"$HOME/.local/bin/couchcordd" doctor || true

say "done. Discord shortcuts should launch ~/.local/bin/game-mode-discord"
say "so each Steam profile gets its own Discord login (see docs/SETUP.md)."
