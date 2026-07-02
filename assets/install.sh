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

# --- 2b. Discord itself ----------------------------------------------------------
# The launcher wraps the flatpak Discord; make sure it exists.
if command -v flatpak >/dev/null 2>&1; then
    if ! flatpak info com.discordapp.Discord >/dev/null 2>&1; then
        say "installing Discord (flatpak)"
        flatpak install -y --noninteractive flathub com.discordapp.Discord
    else
        say "Discord flatpak already installed"
    fi
else
    say "WARNING: flatpak not found — install Discord yourself and adjust the launcher"
fi

# --- 2c. make every launch path multi-tenant -------------------------------------
# App menu / desktop: a user-level desktop entry shadows the flatpak-exported
# one (user dirs precede system dirs in XDG_DATA_DIRS), and flatpak updates
# never touch ~/.local/share/applications — so this survives both Discord and
# OS updates.
install -Dm644 /dev/stdin "$HOME/.local/share/applications/com.discordapp.Discord.desktop" <<EOF
[Desktop Entry]
Name=Discord
Comment=Discord (per-Steam-account profile)
Exec=$HOME/.local/bin/game-mode-discord %U
Icon=com.discordapp.Discord
Type=Application
Categories=Network;InstantMessaging;
MimeType=x-scheme-handler/discord;
StartupWMClass=discord
EOF
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$HOME/.local/share/applications" || true
say "desktop entry (menu + discord:// links) overridden to use the multi-tenant launcher"

# Discord's "open on login" writes an XDG autostart entry that launches the
# flatpak directly — reroute it if present (re-run install.sh if Discord
# recreates it after a settings change).
AUTOSTART="$HOME/.config/autostart/com.discordapp.Discord.desktop"
if [ -f "$AUTOSTART" ] && ! grep -q game-mode-discord "$AUTOSTART"; then
    sed -i "s|^Exec=.*|Exec=$HOME/.local/bin/game-mode-discord|" "$AUTOSTART"
    say "rerouted Discord autostart entry through the launcher"
fi

# Steam shortcuts: rewrite any entry that launches Discord directly. Needs
# Steam closed (it rewrites shortcuts.vdf on exit).
if pgrep -x steam >/dev/null 2>&1; then
    say "Steam is running — skipped shortcut rewire. Close Steam and run:"
    say "  $REPO/assets/rewire-steam-shortcuts.py"
    python3 "$REPO/assets/rewire-steam-shortcuts.py" --dry-run || true
else
    python3 "$REPO/assets/rewire-steam-shortcuts.py" || true
fi

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
