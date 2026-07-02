# Setup (one-time)

## 0. Quick install (Steam Deck / SteamOS / any systemd Linux)
```sh
git clone https://github.com/MasonRhodesDev/couchcord.git
couchcord/assets/install.sh
```
Builds (using distrobox when the host has no compiler — SteamOS ships it),
installs `couchcordd` + the multi-tenant Discord launcher to `~/.local/bin`,
default config, and an enabled systemd user unit that starts with every
graphical session. Idempotent; re-run to upgrade. Steps below are the manual
equivalent plus the one-time bits no installer can do (Steam Input bindings).

## 0.1 Multi-tenancy (shared devices)
All Steam profiles on a Deck run as one Linux user. Two pieces keep tenants
separate:
- **`assets/game-mode-discord`** — point every profile's Discord shortcut at
  this launcher; it binds Discord's config dir to the active Steam account
  (detected via `loginusers.vdf` `MostRecent`), so each profile keeps its own
  Discord login. Flatpak-aware (profile dirs live inside the app's sandbox dir).
- **`couchcordd` tenant state** — the daemon detects the active Steam account
  at startup and namespaces per-user state (future token cache) under
  `~/.local/state/couchcord/tenants/<account_id>`.


## 1. Discord application
- Application ID (`client_id`): `1514871580591919246` — public, non-secret.
  Owner = you, so RPC works without separate Discord approval.
- Put it in `~/.config/couchcord/config.toml` (see `config.toml.example`).
- **Live validation TODO:** confirm `AUTHORIZE → AUTHENTICATE → SELECT_VOICE_CHANNEL`
  works with this app on the account (Phase 2). The auth round-trip may need the
  app's **client secret** for the OAuth code exchange; if so, that's added to
  config when we test live.

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
Run the Discord client (background) in the game-mode session for voice — the
daemon only *controls* it over RPC; it doesn't launch it.

First login per Steam profile: open the Discord tile (the installer creates one
per profile), sign in — the QR-code scan with the phone app is fastest on a
Deck — then approve couchcord's one-time RPC authorization prompt inside
Discord. Desktop mode is covered too: the app-menu entry, `discord://` links,
and any autostart entry all route through the multi-tenant launcher, and the
launcher kills a running instance from another Steam profile before starting
(Discord is single-instance, so a stale instance would leak the previous
user's session).

## 5. Run
```sh
cargo build --release
sudo install -m755 target/release/couchcordd /usr/local/bin/couchcordd
install -m644 assets/systemd/couchcordd.service ~/.config/systemd/user/
systemctl --user enable --now couchcordd          # or: couchcordd run
couchcordd doctor                                  # verify the 3 assumptions in-session
```
