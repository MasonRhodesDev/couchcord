# Setup (one-time)

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
Run the native Discord client (background) in the game-mode session for voice —
the daemon only *controls* it over RPC; it doesn't launch it.

## 5. Install & run
Install the package (see the README "Install" section — `[mason]` pacman repo
on Arch, `solaris765/couchcord` COPR on Fedora). It ships
`/usr/bin/couchcordd`, the user unit in `/usr/lib/systemd/user/`, and
`config.toml.example` under `/usr/share/doc/couchcord/`.

```sh
systemctl --user enable --now couchcordd          # or: couchcordd run
couchcordd doctor                                  # verify the 3 assumptions in-session
```

For hacking on the tree itself, run straight from cargo — no manual
`/usr/local/bin` install step:

```sh
cargo run -p couchcordd -- run       # foreground, current session
```
