# couchcord — spec (working name, renameable)

Controller-driven Discord **voice control + activity overlay** for a gamescope
Big Picture / game-mode session. From the couch, with a controller, while a
game is running: browse and select voice channels, leave, and see who's
talking — **without Discord being a focus-stealing window**.

## Why this exists

In a gamescope Steam Big Picture session, a non-Steam shortcut (how Discord was
previously run) is a "game" to Steam. gamescope shows one surface at a time and,
on game exit, Steam returns focus to the *previous running game* — so launching
Discord-as-a-shortcut creates an inescapable focus trap (exit Overwatch → land
on Discord, no reliable way back to Big Picture). This project replaces that
with a tool that never enters the focus stack.

## Locked success criteria

1. **Background runtime** — native Discord runs hidden for voice; this tool is
   NOT a Steam shortcut and never a gamescope focus-stack surface. Exiting any
   game always returns to Big Picture.
2. **Local official RPC only** — all Discord interaction via the `discord-ipc-0`
   socket. No injection, no Vencord, no web Discord. (One-time personal Discord
   app registration is accepted setup.)
3. **Chord opens the GUI** — a controller chord opens the menu; it grabs input
   and holds focus until dismissed, then releases back to the game.
4. **Steam-client-styled GUI**, drawn over the active window.
5. **Server → voice-channel browser/selector**, filtered to voice channels only
   — includes **Stage channels (`type 13`) in v1** alongside regular voice
   (`type 2`), with the speaker/audience sub-flow.
6. **Leave channel.**
7. **Voice-activity overlay** while connected (who's in / who's speaking),
   anchorable to **8 screen positions** (4 corners + 4 edge midpoints).
8. ~~Soundboard~~ — **dropped.** Not reachable via official API without a bot
   (RPC has no soundboard command; REST `send-soundboard-sound` needs a bot
   identity in voice = "something special", which is disallowed).

## Hard constraints

- **Official Discord API only** (the single constraint). Network requests are
  allowed as long as they hit official Discord surfaces — the **local RPC** (IPC
  socket, primary), the **REST API**, and the **CDN** (`cdn.discordapp.com` for
  icons/avatars) are all fair game. No injection, no Vencord, no web Discord, no
  third-party services. Soundboard stays out only because its official path needs
  a *bot* (a "something special" the user ruled out), not because of the network.
- Primary surface is still the **local RPC**. Confirmed command set on the IPC socket:
  `AUTHORIZE`, `AUTHENTICATE`, `GET_GUILDS`, `GET_GUILD`, `GET_CHANNELS`,
  `GET_CHANNEL`, `SELECT_VOICE_CHANNEL` (join; `channel_id=null` to leave),
  `GET_SELECTED_VOICE_CHANNEL`, `SET_VOICE_SETTINGS`,
  `SET_USER_VOICE_SETTINGS`, `SUBSCRIBE`/`UNSUBSCRIBE` (voice state + speaking
  events). No soundboard command exists.
- **RPC is whitelist-gated** → the user registers their own Discord app (owner
  = implicit tester); the tool uses that `client_id`. Voice-channel filter:
  channel `type == 2` (GUILD_VOICE), optionally `13` (STAGE_VOICE).
- **Language: Rust.**
- **Never a Steam "game" / focus-stack window** (the whole point).

## Key technical findings (de-risked during exploration)

- **Rendering**: a gamescope **external-overlay** X11 window (the
  `GAMESCOPE_EXTERNAL_OVERLAY` atom) draws on top without being a focus-stack
  surface. Proven by `discover-overlay` (Python, GPL) which already renders
  voice activity over gamescope this way and speaks the same RPC. We reuse its
  *techniques*, not its code (we're Rust).
- **Invocation + input (the hard problem, solved)**: route everything through
  **Steam Input as keyboard**. A Steam Input **action-layer**, activated by a
  chord, emits a signal key AND remaps the controller (d-pad→arrows, A→Enter,
  B→Esc). The daemon reads the **virtual keyboard** Steam Input outputs via
  uinput — it never touches the grabbed physical controller, sidestepping the
  `EVIOCGRAB` exclusivity that blocks raw-evdev reads mid-game. Gotcha:
  gamescope masks the left-Windows key — use unmasked keys for chord/nav output.
- **Discord**: runs minimized/background for voice; the tool connects to
  `$XDG_RUNTIME_DIR/discord-ipc-0`.

## Design priorities (from the user, weighted highest)

- **Highly modular** software with **clear service/module boundaries**, so
  **upgrades are easy** and **domains are isolated**. This outranks raw
  simplicity in trade-offs.

## Setup model (one-time, accepted)

- Register a personal Discord application (developer portal) → `client_id`.
  (Done: `1514871580591919246`, see docs/SETUP.md.)
- Import a Steam Input controller template defining the chord + menu action
  layer (shipped with the project).
- Native Discord set to launch/background in the game-mode session.
- **Input-device access via `input`-group membership** (no `/etc/udev` root
  rule): the user is added to the `input` group so the `--user` daemon can read
  the Steam virtual keyboard / uinput.

## Two items that need LIVE validation during build (not blockers)

1. `SELECT_VOICE_CHANNEL` end-to-end with the registered app (whitelist/scope
   path on a real account).
2. The Steam Input template + keyboard flow reaching the daemon mid-game.
