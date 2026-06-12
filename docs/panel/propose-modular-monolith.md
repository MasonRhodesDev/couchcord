# couchcord — Architecture Proposal: The Modular Monolith

**Philosophy (assigned, non-negotiable for this doc):** *A single Rust binary with
strict internal boundaries — a cargo workspace of domain crates behind trait
interfaces, composed in one process and one event loop. Domains stay swappable
behind traits; deployment is one artifact.*

**User's top priority (outranks raw simplicity):** highly modular software,
clear service/module boundaries, easy upgrades, isolated domains.

This document is a build-ready design, not a survey. It is opinionated. Where I
make a call, I say why, and I name the cheaper alternative I rejected.

---

## 0. The one-paragraph thesis

couchcord ships as **one binary, `couchcordd`**, supervised as **one user-level
systemd service**. Inside it is a cargo **workspace of domain crates** that never
call each other directly — they communicate only through (a) **trait objects
injected at composition time** and (b) a **single typed event bus** (`tokio`
broadcast/mpsc) carried on **one async runtime with one logical event loop**. The
binary crate (`couchcord`) is the *only* place that knows concrete types exist; it
wires traits to implementations and owns the `tokio::select!` reactor. Every
domain — Discord RPC, overlay renderer, input source, menu state machine, config —
is a black box behind a trait, with its own error type, its own owned state, and
its own test harness. Swapping a renderer or an input method is a change to *one
crate plus three lines in the composition root*, and the blast radius is provably
bounded because no other crate can name the swapped type.

---

## 1. System Design

### 1.1 Process & service topology

There are **three OS-level things**, and only one of them is ours:

| Thing | Owner | Lifecycle | Why it's separate |
|---|---|---|---|
| **Native Discord** | Discord Inc. | Launched/backgrounded by the game-mode session (Steam library entry or session autostart, minimized) | It owns the `discord-ipc-0` socket. We are a *client* of it. We must NOT manage its lifecycle — that's the focus-trap mistake the SPEC exists to avoid. |
| **`couchcordd`** (us) | systemd `--user` | `WantedBy=graphical-session.target`, `Restart=on-failure` | The whole tool. One binary, always running, never a focus-stack surface. |
| **Steam + Steam Input** | Valve | The game-mode session | Produces our input. We consume its uinput virtual-keyboard output. We never touch the physical pad. |

**Decision: one daemon, systemd `--user`, NOT wrapper-spawned.**

Rejected alternative: spawning `couchcordd` as a child of a game launch wrapper
(`%command%`). That re-introduces exactly the failure the SPEC describes — a
process tied to a game's lifecycle, dying on game exit, with focus-stack
ambiguity. We must be **session-scoped, not game-scoped**. The daemon starts with
the graphical session and lives across every game launch/exit. This is the single
most important systemic decision and it falls straight out of success criterion 1.

```
# ~/.config/systemd/user/couchcordd.service  (shipped, installed by `couchcordd install`)
[Unit]
Description=couchcord Discord couch-control daemon
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=notify                     # sd_notify READY=1 once event loop + RPC handshake settle
ExecStart=%h/.local/bin/couchcordd run
Restart=on-failure
RestartSec=2
# Hardening: we only need uinput, X11, and the discord IPC socket
NoNewPrivileges=true
# uinput access via udev rule (see 1.4), not CAP_SYS_ADMIN

[Install]
WantedBy=graphical-session.target
```

`Type=notify` matters: the daemon reports `READY=1` only after the input device,
the overlay window, and config are up (RPC handshake is allowed to be lazy — see
§1.3 reconnection). This gives systemd a real health signal and makes
`systemctl --user status couchcordd` meaningful.

### 1.2 Inside the process: one runtime, one event loop, many domains

```
                         ┌──────────────────────────────────────────────┐
                         │            couchcord (binary crate)            │
                         │     composition root + tokio reactor           │
                         │                                                │
                         │   ┌────────────── EVENT BUS ──────────────┐    │
                         │   │ tokio broadcast<Event> + mpsc<Command>│    │
                         │   └───▲────────▲────────▲────────▲────────┘    │
                         │       │        │        │        │             │
   uinput  ──────────────┼──► [input] [menu-state] [discord-rpc] [overlay-render]
   (virtual kbd)         │     impl     impl        impl          impl    │
                         │   InputSource MenuEngine RpcClient   OverlayRenderer
                         │       │        │        │        │             │
                         │       └────────┴───[config]┴────[core types]   │
                         └──────────────────────────────────────────────┘
                                              │
   X11 external-overlay window ◄──────────────┘ (GAMESCOPE_EXTERNAL_OVERLAY)
   discord-ipc-0 unix socket   ◄───────────────────────────► native Discord
```

**There is exactly one async runtime (`tokio` multi-thread) and one logical
reactor** — a single `tokio::select!`-driven supervisor task in the binary crate.
Each domain runs as a **supervised task** (or a small set of tasks) owned by the
reactor. Domains do not spawn unsupervised threads; they hand the reactor a
`Future` or expose a `poll`-style step the reactor drives. This is what "one
process and one event loop" means concretely, and it's also what makes the system
*reasoned-about*: there is one place where everything is composed and ordered.

### 1.3 IPC / communication — internal and external

**Internal (between domains): the typed event bus. No domain ever holds a
concrete handle to another domain.** Two channels:

- `Event` — facts that already happened (`broadcast`, fan-out, lossy-tolerant):
  `VoiceStateUpdated`, `SpeakingStarted/Stopped`, `ChordPressed`, `NavInput`,
  `GuildsLoaded`, `RpcConnected/Disconnected`, `MenuOpened/Closed`.
- `Command` — requests to *do* something (`mpsc`, addressed, back-pressured):
  `OpenMenu`, `JoinVoice(channel_id)`, `LeaveVoice`, `MoveOverlay(Anchor)`,
  `FetchChannels(guild_id)`.

Why two? It enforces a **direction of dependency**: domains *emit* events upward
to the bus and *receive* commands downward from the bus, but never know who
produced or consumes them. The menu-state engine turns input Events into
Commands; the binary routes Commands to the right domain trait. This is the
classic ports-and-adapters seam, and it's the mechanism that makes domains
isolated.

**External IPC #1 — Discord:** Unix domain socket `$XDG_RUNTIME_DIR/discord-ipc-0`,
framed with Discord's IPC framing (4-byte LE opcode, 4-byte LE length, JSON
payload). Lives **entirely** inside the `discord-rpc` crate. The rest of the
system never sees a byte of JSON or a socket — only `core` domain types.
Reconnection is the `discord-rpc` crate's private problem: it owns an exponential
backoff loop and emits `RpcConnected`/`RpcDisconnected` events so the UI can show
"Discord not running."

**External IPC #2 — input:** the `input` crate opens the **uinput-created virtual
keyboard** that Steam Input emits (by device name match, e.g.
`"Steam Controller"`/the virtual keyboard node), reads evdev key events, and
translates *only* the signal/nav keys into `Event::ChordPressed` /
`Event::NavInput(Up|Down|Left|Right|Confirm|Cancel)`. It **never** opens or
`EVIOCGRAB`s the physical controller — that's the SPEC's hard-won insight, and it
becomes a one-crate invariant.

**External IPC #3 — rendering:** the `overlay-render` crate owns an X11
connection (via `x11rb`), creates one override-redirect window, sets the
`GAMESCOPE_EXTERNAL_OVERLAY` property so gamescope composites it on top without
adding it to the focus stack, and paints with a software/Cairo or GPU surface. The
window is created once and shown/hidden/repositioned; it is never destroyed on
menu-close (cheap show/hide, and it keeps the voice-activity overlay alive
independent of the menu).

### 1.4 Deployment — one artifact

- **Build:** `cargo build --release` → a single static-ish binary `couchcordd`
  (dynamically links libX11/libxcb and libc; everything else is in-binary).
- **Install:** `couchcordd install` (a subcommand, not a separate tool) writes the
  systemd unit, a **udev rule** granting the user access to `/dev/uinput` and the
  Steam virtual keyboard, the **Steam Input controller template** (`.vdf`), and a
  default `config.toml`. `couchcordd doctor` validates all of it (socket present?
  uinput readable? overlay atom supported? client_id set?).
- **Upgrade:** replace one file, `systemctl --user restart couchcordd`. Because the
  artifact is one binary, there is no version-skew surface between components —
  the modularity is *internal*, the deployment is *atomic*. This is the core
  trade the philosophy buys us: maximum internal swappability, zero distributed-
  systems operational cost.

### 1.5 Lifecycle & supervision

- **Process supervision:** systemd (`Restart=on-failure`, `RestartSec=2`,
  `WatchdogSec` via `sd_notify` heartbeats from the reactor — if the event loop
  wedges, systemd kills and restarts us).
- **Task supervision (in-process):** the reactor owns a `JoinSet`. If a domain
  task panics or returns `Err`, the supervisor logs it, emits a
  `DomainFailed(name)` event (so the overlay can show a degraded badge), and
  **restarts just that domain task** with backoff. A dead `discord-rpc` task does
  not take down `input` or `overlay-render`. This is "let it crash" scoped to one
  domain — isolation at runtime, not just at compile time.

---

## 2. Software Design — the cargo workspace

```
couchcord/
├─ Cargo.toml                 # [workspace]
├─ crates/
│  ├─ core/                   # shared domain types + the event bus contract. NO logic.
│  ├─ config/                 # load/validate/watch config; owns config schema
│  ├─ discord-rpc/            # the only crate that knows the IPC socket & JSON
│  ├─ input/                  # the only crate that knows uinput/evdev
│  ├─ overlay-render/         # the only crate that knows X11/gamescope/drawing
│  ├─ menu-state/             # pure state machine: Events -> view model + Commands
│  └─ couchcord/  (bin)       # composition root + reactor. Knows ALL concrete types.
└─ ...
```

### 2.1 The dependency rule (the spine of the modularity)

```
        core  ◄──────────────── everyone depends on core, core depends on no one
         ▲
   ┌─────┼───────┬───────────┬──────────────┐
config  discord-rpc  input  overlay-render  menu-state    (siblings: NEVER depend on each other)
   ▲     ▲          ▲        ▲               ▲
   └─────┴──────────┴────────┴───────────────┘
                     couchcord (bin)   ◄── the ONLY crate that depends on impl crates
```

**Enforced invariant:** sibling domain crates have *zero* dependencies on each
other. They depend only on `core` (types + traits) and the `tokio` channels in
`core`. The binary is the only crate allowed to `use discord_rpc::IpcRpcClient`.
This is enforceable mechanically (a `cargo-deny`/`x-task` check that fails CI if a
domain crate's `Cargo.toml` names a sibling). That single rule is what makes blast
radius computable.

### 2.2 `core` — types and contracts (no behavior)

Owns the vocabulary every domain speaks, plus the event/command enums and the
trait definitions. Changing `core` is the one thing that *can* touch everyone, so
`core` is deliberately tiny and stable.

```rust
// core/src/model.rs  — domain types, all `#[non_exhaustive]` to allow additive evolution
pub struct GuildId(pub u64);
pub struct ChannelId(pub u64);
pub struct UserId(pub u64);

pub struct Guild   { pub id: GuildId, pub name: String, pub icon: Option<IconRef> }
pub struct Channel { pub id: ChannelId, pub name: String, pub kind: ChannelKind }
#[non_exhaustive]
pub enum ChannelKind { GuildVoice, StageVoice, Other }   // map of type==2 / type==13

pub struct VoiceMember { pub user: UserId, pub name: String, pub speaking: bool,
                         pub muted: bool, pub deafened: bool }

#[non_exhaustive]
pub enum Anchor { TopLeft, TopCenter, TopRight, MiddleLeft, MiddleRight,
                  BottomLeft, BottomCenter, BottomRight }   // the 8 positions, criterion 7
```

```rust
// core/src/event.rs  — the bus vocabulary
#[non_exhaustive]
pub enum Event {
    // input domain
    ChordPressed,
    Nav(NavInput),                       // Up/Down/Left/Right/Confirm/Cancel/MoveOverlay
    // discord-rpc domain
    RpcConnected, RpcDisconnected,
    GuildsLoaded(Vec<Guild>),
    ChannelsLoaded { guild: GuildId, channels: Vec<Channel> },
    VoiceStateUpdated { channel: ChannelId, members: Vec<VoiceMember> },
    SpeakingChanged { user: UserId, speaking: bool },
    SelectedVoiceChannel(Option<ChannelId>),
    // menu domain
    ViewModelChanged(ViewModel),         // what the renderer should draw
    DomainFailed(&'static str),
}

#[non_exhaustive]
pub enum Command {
    OpenMenu, CloseMenu,
    FetchGuilds, FetchChannels(GuildId),
    JoinVoice(ChannelId), LeaveVoice,
    MoveOverlay(Anchor),
    Subscribe(GuildId), Unsubscribe(GuildId),
}
```

`#[non_exhaustive]` everywhere is a deliberate upgrade lever: a new RPC capability
or a new nav action is an **additive** change; downstream `match`es keep compiling
(they already have a `_ =>` arm by force of `#[non_exhaustive]`).

### 2.3 The four boundary traits (one per swappable domain)

These are the load-bearing interfaces. Each is small, async where it touches IO,
and returns a **domain-specific error** so failures don't leak representation.

```rust
// === discord-rpc boundary =================================================
// Single responsibility: speak official local RPC; expose nothing about JSON/sockets.
#[async_trait]
pub trait RpcClient: Send + Sync + 'static {
    async fn connect(&self, app: ClientId) -> Result<(), RpcError>;
    async fn guilds(&self) -> Result<Vec<Guild>, RpcError>;
    async fn channels(&self, guild: GuildId) -> Result<Vec<Channel>, RpcError>; // caller filters voice
    async fn select_voice(&self, channel: Option<ChannelId>) -> Result<(), RpcError>; // None == leave
    async fn selected_voice(&self) -> Result<Option<ChannelId>, RpcError>;
    /// Long-lived: emits VoiceStateUpdated / SpeakingChanged onto the bus.
    fn subscribe_voice(&self, guild: GuildId) -> BoxStream<'static, VoiceEvent>;
}

// === input boundary =======================================================
// Single responsibility: turn the Steam-Input virtual keyboard into nav semantics.
pub trait InputSource: Send + 'static {
    /// A stream of high-level intents. The impl owns evdev/uinput entirely.
    fn intents(&mut self) -> BoxStream<'static, InputIntent>; // Chord, Nav(dir), Confirm, Cancel...
    /// Grab/ungrab is a no-op concept here: we read a virtual device, never EVIOCGRAB.
    fn set_active(&mut self, capturing: bool);                // when menu open, capture nav keys
}

// === overlay-render boundary ==============================================
// Single responsibility: paint a ViewModel onto the gamescope external overlay.
pub trait OverlayRenderer: Send + 'static {
    fn ensure_window(&mut self) -> Result<(), RenderError>;   // create override-redirect + set atom
    fn render(&mut self, vm: &ViewModel) -> Result<(), RenderError>;
    fn set_anchor(&mut self, anchor: Anchor);                 // the 8-position reposition
    fn set_visible(&mut self, visible: bool);                 // menu show/hide; overlay stays alive
}

// === config boundary ======================================================
pub trait ConfigSource: Send + Sync + 'static {
    fn current(&self) -> Config;                              // client_id, theme, default anchor, keymap
    fn watch(&self) -> BoxStream<'static, Config>;            // hot-reload on file change
}
```

Note what is *not* in these traits: no JSON, no `xcb::Window`, no `evdev::Device`,
no socket. Each trait is the complete, sufficient contract for its domain. The
binary depends on the traits; the impls are injected.

### 2.4 `menu-state` — the pure brain (no IO, no traits-to-impls)

The single most testable crate. It is a **deterministic state machine**:
`(State, Event) -> (State, Vec<Command>, ViewModel)`. It owns the menu UX:
which screen, current selection, voice-channel filtering, the 8-position cursor.
It has **no dependency on any IO crate** — it can't, by the dependency rule — so
it is unit-tested with plain enums and zero mocking. This is where success
criteria 3/5/6/7 are *decided*; the IO crates merely execute.

```rust
pub enum Screen { Closed, Guilds, Channels(GuildId), InVoice(ChannelId), Reposition }

pub struct MenuEngine { screen: Screen, guilds: Vec<Guild>, /* selection, cache */ }

impl MenuEngine {
    /// Pure. No `await`, no IO. Returns commands for the binary to dispatch.
    pub fn handle(&mut self, ev: &Event) -> Step;   // Step { commands: Vec<Command>, view: ViewModel }
}
```

### 2.5 The composition root — `couchcord` (bin)

The only crate that imports concrete impls. ~150 lines. It:

1. constructs each impl (`IpcRpcClient::new()`, `UinputSource::open()`,
   `X11Overlay::new()`, `FileConfig::load()`),
2. binds them to `Box<dyn Trait>`,
3. creates the bus,
4. runs the reactor:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg:    Box<dyn ConfigSource>    = Box::new(config::FileConfig::load()?);
    let rpc:    Box<dyn RpcClient>       = Box::new(discord_rpc::IpcRpcClient::new());
    let input:  Box<dyn InputSource>     = Box::new(input::UinputSource::open(&cfg.current())?);
    let render: Box<dyn OverlayRenderer> = Box::new(overlay_render::X11Overlay::new()?);
    let mut menu = menu_state::MenuEngine::new();

    let (events, commands) = bus::new();
    reactor::run(cfg, rpc, input, render, &mut menu, events, commands).await
}
```

Swapping any domain = change one `Box::new(...)` line. That's the upgrade story
made literal.

### 2.6 Data / event flow for the required interactions

Notation: `[input]→bus` means a domain emits an Event onto the bus; `bus→[x]`
means the binary routed a Command to domain x.

**Open menu (criterion 3):**
```
Steam Input chord → virtual kbd signal key
[input] reads key → Event::ChordPressed → bus
reactor → menu.handle(ChordPressed) → Step{ cmds:[OpenMenu, FetchGuilds, FetchGuilds-sub], view:Guilds(loading) }
reactor dispatches: [input].set_active(true)   // now capture nav keys
                    [overlay-render].set_visible(true) + render(loading view)
                    [discord-rpc].guilds()  (async; result later)
```
`set_active(true)` is the "grab input, hold focus" of criterion 3 — but it's a
*logical* capture of the virtual keyboard's nav keys, not an X grab and not an
EVIOCGRAB. On CloseMenu we `set_active(false)` and `set_visible(false)`; the game
never lost real focus, so we "release back to the game" trivially (criterion 1+3).

**Browse servers (criterion 5, part 1):**
```
[discord-rpc].guilds() resolves → Event::GuildsLoaded(vec) → bus
reactor → menu.handle(GuildsLoaded) → Step{ view:Guilds(list) }
reactor → [overlay-render].render(Guilds view)
Nav(Up/Down) from [input] → menu moves selection → render. (pure, instant)
```

**List voice channels (criterion 5, part 2):**
```
Nav(Confirm) on a guild → menu → Step{ cmds:[FetchChannels(g), Subscribe(g)] }
reactor → [discord-rpc].channels(g)
[discord-rpc] returns ALL channels → reactor passes to menu →
   menu FILTERS to ChannelKind::{GuildVoice, StageVoice}  (criterion: type==2 / 13)
   → Step{ view:Channels(voice_only) } → render
```
The voice filter lives in `menu-state` (policy), not `discord-rpc` (mechanism) —
so if Discord adds a new voice type, it's a one-line policy change, no RPC change.

**Select voice channel (criterion: SELECT_VOICE_CHANNEL):**
```
Nav(Confirm) on a channel → menu → Step{ cmds:[JoinVoice(ch)] }
reactor → [discord-rpc].select_voice(Some(ch))
[discord-rpc] also receives VoiceStateUpdated via its subscription stream →
   Event::VoiceStateUpdated → menu → Screen::InVoice → render member list
```

**Leave (criterion 6):**
```
Nav(Cancel/Leave) in InVoice → menu → Step{ cmds:[LeaveVoice] }
reactor → [discord-rpc].select_voice(None)    // channel_id=null == leave
→ Event::SelectedVoiceChannel(None) → menu → back to Channels/Guilds → render
```

**Render voice activity (criterion 7, part 1):**
```
[discord-rpc] subscription stream (SUBSCRIBE voice_state + speaking) →
   Event::VoiceStateUpdated / SpeakingChanged → bus
reactor → menu folds into ViewModel (who's in / who's speaking) → 
   [overlay-render].render(vm)
This path runs even when the MENU is CLOSED: the overlay stays visible while
connected. set_visible(menu) and the activity overlay are independent layers in
the ViewModel.
```

**Reposition overlay — 8 positions (criterion 7, part 2):**
```
In Reposition screen, Nav(Left/Right) cycles Anchor (or a 3x3 grid pick) →
   menu → Step{ cmds:[MoveOverlay(anchor)] } 
reactor → [overlay-render].set_anchor(anchor)  // recompute x,y from screen geom
config persists chosen anchor via [config] so it survives restart.
```

---

## 3. How it meets each locked success criterion

| # | Criterion | How this design satisfies it |
|---|---|---|
| **1** | Background runtime, never a focus-stack surface | systemd `--user` session-scoped daemon (§1.1), NOT a Steam shortcut and NOT wrapper-spawned. The overlay window sets `GAMESCOPE_EXTERNAL_OVERLAY` so gamescope composites it without adding it to the focus stack (§1.3 IPC#3). Game exit always returns to Big Picture because we were never in the stack. |
| **2** | Local official RPC only | All Discord contact is confined to `discord-rpc` over `discord-ipc-0` with the official command set. No injection/Vencord/web — those would require a different crate that simply does not exist in the workspace. `client_id` from config (§2.2). |
| **3** | Chord opens GUI, grabs input, releases on dismiss | `input` reads the Steam-Input virtual keyboard chord → `ChordPressed` → menu opens, `set_active(true)` logically captures nav keys; on close `set_active(false)` releases. No X grab, no game focus loss (§2.6 Open menu). |
| **4** | Steam-client-styled GUI over active window | `overlay-render` owns drawing with a Steam-styled theme (dark, rounded, Steam accent) on the external-overlay window, composited over whatever's active (§1.3, §2.3). Theme tokens come from `config`. |
| **5** | Server → voice-channel browser, voice-only | Guild browse + channel fetch via `discord-rpc`; **voice-only filtering is policy in `menu-state`** (`ChannelKind::GuildVoice/StageVoice`, type 2/13). (§2.6 browse/list.) |
| **6** | Leave channel | `select_voice(None)` (`channel_id=null`) in the `RpcClient` trait; one Command (`LeaveVoice`). (§2.3, §2.6 Leave.) |
| **7** | Voice-activity overlay, 8 anchors | Subscription-driven `VoiceStateUpdated`/`SpeakingChanged` → ViewModel → renderer, live while connected and independent of the menu. `Anchor` enum has exactly the 8 positions; `set_anchor` repositions; choice persisted by `config`. (§2.2, §2.6 render/reposition.) |

(Criterion 8, soundboard, is correctly dropped — there is simply no trait method
for it, which is the cleanest possible way to encode "out of scope.")

---

## 4. Upgrade & Isolation — the headline feature

The user weighted this highest. Here is the concrete payoff, per domain, with
**blast radius** stated as "files that must change."

### 4.1 General mechanism

Three layered guarantees:

1. **Compile-time isolation:** sibling crates can't name each other (dependency
   rule, CI-enforced). So a change inside `discord-rpc` *cannot* break
   `overlay-render` — there is no edge between them to break.
2. **Type-level stability:** all bus/model types are `#[non_exhaustive]`, so
   additive evolution never forces downstream edits.
3. **Injection at one point:** concrete types appear only in the binary, so
   *replacing* an impl is a single `Box::new` line, not a refactor.

### 4.2 Blast radius: a **Discord-RPC change**

*Scenario: Discord ships a new RPC field, or you move from raw IPC to a
maintained `discord-rich-presence`-style lib, or the framing changes.*

- **Changes:** `crates/discord-rpc/` internals only. If the `RpcClient` trait
  signature is unchanged → **blast radius = 1 crate, 0 other files.**
- If a *new capability* is added (say `MoveMember`): add a trait method
  (default-impl returning `Unsupported` to stay non-breaking) + one `Command`
  variant in `core` (additive, `#[non_exhaustive]`) + wire it in the binary.
  **Blast radius = 3 files**, all additive, nothing breaks.
- `menu-state`, `overlay-render`, `input` **do not recompile-break**: they never
  saw the socket. The voice filter, the UI, the input all keep working.

### 4.3 Blast radius: a **renderer swap**

*Scenario: replace X11/Cairo software rendering with a GPU `wgpu` renderer, or
move from gamescope external-overlay to a Wayland layer-shell surface on a
non-gamescope session.*

- **Changes:** add `crates/overlay-render-wgpu/` (or edit the existing one)
  implementing `OverlayRenderer`. Flip **one line** in the binary:
  `Box::new(overlay_render_wgpu::WgpuOverlay::new()?)`.
- The `ViewModel` contract is unchanged, so `menu-state` is untouched. The window
  protocol (X11 vs Wayland layer-shell) is *entirely* behind the trait — the rest
  of the system doesn't know what windowing system exists. **Blast radius = 1 new
  crate + 1 line.** You can even keep both and pick by config/runtime detection.

### 4.4 Blast radius: an **input-method change**

*Scenario: Steam Input changes its virtual-keyboard behavior; or you add a
fallback path (a real evdev grab when not mid-game, or a network/phone trigger,
or a global hotkey on a desktop session).*

- **Changes:** new impl of `InputSource` in `crates/input/` (or a sibling
  `input-evdev`). The trait yields `InputIntent`s — *semantics*, not key codes —
  so the menu's nav logic is immune to how the intent was produced.
- **Blast radius = 1 crate + 1 binary line.** The EVIOCGRAB-avoidance invariant
  is documented and localized; a new input source either honors it or is a
  knowingly different strategy, but either way no other domain is touched.

### 4.5 Why "easy upgrades" is real here, not aspirational

The deployment is one binary, so an upgrade is `cargo build && systemctl --user
restart` — no migration, no two-component version matrix, no IPC schema
negotiation between separately-deployed services. You get **distributed-systems-
grade internal modularity with zero distributed-systems operational tax.** That is
precisely the trade the assigned philosophy is *good* at, and it lines up with the
user's stated ranking (modularity/isolation > raw simplicity).

---

## 5. The 3 biggest honest RISKS / weaknesses of THIS approach for THIS tool

**Risk 1 — Shared-process fate-sharing undercuts the isolation promise.**
Compile-time isolation is excellent; *runtime* isolation is only as good as the
supervisor. A panic in `overlay-render` (an X11 hiccup mid-game) can poison the
whole process if it isn't caught. We mitigate with per-domain supervised tasks +
`catch_unwind` boundaries + systemd restart, but the honest truth is that a true
multi-process design (separate renderer process) would give *hard* memory
isolation we don't have. We're betting that disciplined task supervision is
"isolated enough," and for a single-user couch tool that bet is reasonable — but
it is a bet. A C FFI call into libX11 that segfaults takes everyone down, period.

**Risk 2 — One async runtime means one place to wedge, and the trait boundaries
add real friction against the messy realities of X11 + uinput + a stateful
socket.** X11 (`x11rb`) is fundamentally a synchronous, connection-stateful API;
uinput/evdev is blocking; the Discord IPC socket is stateful and order-sensitive.
Forcing all three behind clean async traits on one reactor invites subtle
foot-guns: a blocking X11 round-trip stalling the reactor, or back-pressure on the
event bus stalling input during a render storm. We isolate blocking work onto
`spawn_blocking`/dedicated threads, but the abstraction tax is real and the
"single event loop" can become a single point of latency coupling. A simpler
design with three independent threads and channels might actually be *easier* to
keep responsive — we're trading some operational simplicity for the architectural
purity the philosophy demands.

**Risk 3 — The modular structure is heavy relative to the validated unknowns,
and over-investing in seams before the two LIVE-validation items land is a real
hazard.** The SPEC flags two things that are *not yet proven on real hardware*:
end-to-end `SELECT_VOICE_CHANNEL` with the registered app, and the Steam-Input
template reaching the daemon mid-game. If either behaves differently than assumed
(e.g., the whitelist/scope path forces an OAuth dance, or gamescope masks more
keys than expected), the **shape** of the `RpcClient` or `InputSource` trait could
need to change — and we'll have built six crates and a bus around assumptions.
The modular design *contains* that damage well (that's its job), but there's a
chicken-and-egg cost: the abstraction boundaries are most valuable *after* the
domains are understood, and here two domains are still partly unknown. The
pragmatic hedge is to spike both validations against thin, ugly versions of the
two impls *before* hardening their traits — design the seam last for those two,
first for the three we already understand (overlay, menu, config).

---

## Appendix A — build order (so the seams are designed against reality where it matters)

1. `core` + `config` + `menu-state` (pure, fully testable today; lock these traits).
2. `overlay-render` spike: get a gamescope external-overlay window painting a
   static ViewModel (proves criterion 1 + 4 + 7-rendering early).
3. `discord-rpc` spike: connect, `GET_GUILDS`, `SELECT_VOICE_CHANNEL`
   (**LIVE validation item 1** — design the trait *after* this works).
4. `input` spike: read the Steam-Input virtual keyboard mid-game
   (**LIVE validation item 2** — design the trait *after* this works).
5. Compose in `couchcord`, add supervision, ship `install`/`doctor`.

Lock the three understood traits first; finalize the two risky traits only after
their spikes prove behavior. This keeps the modularity an asset, not a guess.
