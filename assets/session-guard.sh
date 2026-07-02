#!/bin/bash
# couchcord session guard: align Discord's lifetime with Steam tenancy.
#
#  - Steam exits inside game mode (gamescope session) -> close Discord: the
#    session is over and the next profile must not inherit this login.
#  - The active Steam account changes while Discord is running (a desktop-mode
#    re-login) -> close and reopen Discord under the new tenant.
#  - Steam exits in desktop mode -> leave Discord alone (same person; only one
#    human can be at a desktop session anyway).
#
# Needed because flatpak apps launched from Steam run in their own systemd
# scopes and survive Steam, the session, and profile switches on their own.

POLL=${COUCHCORD_GUARD_POLL:-5}
LOGINUSERS="${COUCHCORD_LOGINUSERS:-$HOME/.steam/steam/config/loginusers.vdf}"
DRYRUN=${COUCHCORD_GUARD_DRYRUN:-}

log() { echo "$*"; }
act() { if [ -n "$DRYRUN" ]; then echo "DRYRUN: $*"; else "$@"; fi; }

active_uid() {
    python3 - "$LOGINUSERS" <<'PY' 2>/dev/null
import re, sys
try:
    d = open(sys.argv[1], encoding='utf-8', errors='replace').read()
except OSError:
    raise SystemExit
for m in re.finditer(r'"(\d{17})"\s*\{(.*?)\}', d, re.S):
    if re.search(r'"MostRecent"\s*"1"', m.group(2)):
        print(int(m.group(1)) - 76561197960265728)
        break
PY
}

discord_running() { pgrep -f "app/com.discordapp.Discord" >/dev/null 2>&1; }
steam_running() { pgrep -x steam >/dev/null 2>&1; }

# Desktop of the active seat session: "gamescope" in game mode, "KDE" (etc.)
# in desktop mode, empty during transitions.
active_desktop() {
    local s d a
    for s in $(loginctl list-sessions --no-legend 2>/dev/null | awk '{print $1}'); do
        a=$(loginctl show-session "$s" -p Active --value 2>/dev/null)
        d=$(loginctl show-session "$s" -p Desktop --value 2>/dev/null)
        if [ "$a" = "yes" ] && [ -n "$d" ]; then
            echo "$d"
            return
        fi
    done
}

last_uid=$(active_uid)
steam_was=""
steam_running && steam_was=1

while true; do
    uid=$(active_uid)
    if [ -n "$uid" ] && [ -n "$last_uid" ] && [ "$uid" != "$last_uid" ] && discord_running; then
        log "steam account changed ($last_uid -> $uid) — restarting Discord for the new tenant"
        act flatpak kill com.discordapp.Discord
        for _ in $(seq 1 20); do discord_running || break; sleep 0.5; done
        # systemd-run: Discord must not be a child of this guard
        act systemd-run --user --collect "$HOME/.local/bin/game-mode-discord"
    fi
    [ -n "$uid" ] && last_uid=$uid

    if steam_running; then
        steam_was=1
    elif [ -n "$steam_was" ]; then
        steam_was=""
        d=$(active_desktop)
        case "$d" in
            gamescope|"")
                log "steam exited (session: ${d:-none}) — closing Discord"
                discord_running && act flatpak kill com.discordapp.Discord
                ;;
            *)
                log "steam exited in desktop session ($d) — leaving Discord running"
                ;;
        esac
    fi
    sleep "$POLL"
done
