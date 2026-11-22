# Setup (one-time)

## 1. Discord application
- Application ID (`client_id`): `1514871580591919246` — public, non-secret.
  Owner = you, so RPC works without separate Discord approval.
- Put it in `~/.config/couchcord/config.toml` (see `config.toml.example`).
- The READY handshake with this app has been verified on a Steam Deck against
  the flatpak client. **Live validation TODO:** confirm
  `AUTHORIZE → AUTHENTICATE → SELECT_VOICE_CHANNEL` works with this app on the
  account (Phase 2). The auth round-trip may need the app's **client secret**
  for the OAuth code exchange; if so, that's added to config when we test live.

## 2. Input access (`input` group)
The daemon reads the Steam-Input virtual keyboard via evdev. Grant access:
```sh
sudo usermod -aG input $USER   # then log out / back in
```
`couchcordd doctor` flags this if it's missing.

## 3. Steam Input bindings (the input gate)
The menu is driven entirely through Steam-Input-emitted **keyboard** keys, so the
daemon never fights the controller grab. In the controller config for game-mode
(or a shared template), add an **action-set layer** "couchcord" activated by a
chord (e.g. hold Back/Select + a face button or a rear paddle), emitting these
**gamescope-unmasked** keys (left-Windows is masked — avoid it):

| Gesture / button (while layer active) | Emits key | Daemon intent |
|---|---|---|
| chord (activates layer)               | `F13`        | open menu |
| D-pad up / down                       | `Up` / `Down`| move cursor |
| D-pad left / right                    | `Left`/`Right`| back / confirm |
| A                                     | `Enter`      | confirm |
| B                                     | `Esc`        | back |
| chord release / Y                     | `Backspace`  | dismiss |
| a spare button                        | `Tab`        | cycle overlay position |

(These match `cc-input::keyname_for_evdev`.) Authoring this in the Steam UI is the
one fiddly manual step; a shippable `.vdf` template is a follow-up.

## 4. Discord client
Run the Discord client (background) in the game-mode session for voice. The
daemon only *controls* it over RPC and never launches it. The native, flatpak
(`com.discordapp.Discord`) and snap clients all work, because the daemon looks
for `discord-ipc-N` under `$XDG_RUNTIME_DIR` and under the flatpak and snap
export dirs inside it.

The daemon can start before Discord. It waits on inotify for the socket to
appear, and a signed-out Discord holds the handshake until someone signs in,
so the sign-in itself completes the connection. If Discord quits or restarts,
the daemon goes back to waiting for the socket. A rejected handshake (wrong
`client_id`, for example) is not retried until Discord recreates its socket,
so fix the config and restart Discord or `couchcordd`. The Steam Input
keyboard is found the same way: when it disappears, the daemon waits for the
next change in `/dev/input`.

## 5. Install & run
Install the package (see the README "Install" section: the `[mason]` pacman
repo on Arch, the `solaris765/couchcord` COPR on Fedora). It ships
`/usr/bin/couchcordd`, `/usr/bin/game-mode-discord`, the user units in
`/usr/lib/systemd/user/`, and `config.toml.example` under
`/usr/share/doc/couchcord/`.

```sh
systemctl --user enable --now couchcordd          # or: couchcordd run
couchcordd doctor                                  # verify the 3 assumptions in-session
```

For hacking on the tree itself, run straight from cargo with no manual
`/usr/local/bin` install step:

```sh
cargo run -p couchcordd -- run       # foreground, current session
```

## 6. Shared devices (Steam Deck, SteamOS)
Every Steam profile on a Deck runs as the same Linux user, so by default every
profile shares one Discord login. Two pieces keep them apart.

- **`game-mode-discord`** launches the Discord flatpak for the Steam account
  that is signed in (the `MostRecent` entry in `loginusers.vdf`). When
  [deck-tenant](https://github.com/MasonRhodesDev/deck-tenant) is installed
  and Discord is registered with it, the launcher hands off to deck-tenant's
  wrapper, which gives each Steam account its own copy of the Discord app
  data and closes Discord on Steam exit and account switches. Without
  deck-tenant, the launcher swaps Discord's config dir per account itself and
  closes a running Discord that belongs to another account before starting.
  The first run moves an existing Discord login to a `default` profile, so
  only use it on a shared device.
- **`couchcord-autostart-guard.path`** watches Discord's autostart entry in
  `$XDG_CONFIG_HOME/autostart`. Discord rewrites that entry whenever "open on
  login" is toggled, pointing it straight at the flatpak. The guard points it
  back at `game-mode-discord` within seconds, and leaves an entry that already
  uses `game-mode-discord` or deck-tenant's `tenant-discord` alone.

SteamOS has a read-only root, so install couchcord into the same user-level
pacman root as deck-tenant. Set that root up first with the steps in
deck-tenant's README, which also put the root's `usr/bin` on `PATH`. Then:

```sh
ROOT=~/.local/share/deck-pkgs
unshare -r pacman --config ~/.config/deck-pkgs/pacman.conf \
    --root $ROOT --dbpath $ROOT/var/lib/pacman \
    --assume-installed gcc-libs --assume-installed systemd \
    --assume-installed bash --assume-installed python \
    -Sy couchcord

install -Dm644 $ROOT/usr/share/doc/couchcord/config.toml.example \
    ~/.config/couchcord/config.toml
systemctl --user link $ROOT/usr/lib/systemd/user/couchcordd.service \
    $ROOT/usr/lib/systemd/user/couchcord-autostart-guard.path \
    $ROOT/usr/lib/systemd/user/couchcord-autostart-guard.service
mkdir -p ~/.config/systemd/user/couchcordd.service.d
printf '[Service]\nExecStart=\nExecStart=%s/usr/bin/couchcordd run\n' "$ROOT" \
    > ~/.config/systemd/user/couchcordd.service.d/root.conf
systemctl --user daemon-reload
systemctl --user enable --now couchcordd couchcord-autostart-guard.path

deck-tenant register --app-id com.discordapp.Discord --name Discord
deck-tenant-steam-sync      # with Steam closed: a Discord tile in every profile
```

The drop-in is needed because the packaged unit runs `/usr/bin/couchcordd`,
which does not exist outside the user root. Upgrade with the same `pacman -Sy`
line. To uninstall, run it with `-R couchcord` and remove the linked units and
the drop-in.

For the first login in each Steam profile, open the Discord tile, sign in (the
QR code scan from the phone app is quickest on a Deck), then approve
couchcord's one-time RPC authorization prompt inside Discord. Set
`COUCHCORD_LOGINUSERS` to point both the daemon and the launcher at a
`loginusers.vdf` outside `~/.steam/steam/config`.
