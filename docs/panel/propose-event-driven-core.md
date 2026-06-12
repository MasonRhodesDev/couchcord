# couchcord — Architecture Proposal: A Typed Event Bus as the Unit of Modularity

**Philosophy (the one we commit to here):** a small, typed, in-process event bus
is *the* unit of modularity. Every capability — reading Steam-Input keystrokes,
talking to Discord over RPC, drawing the gamescope overlay, deciding what the
menu does next — is a **module that owns a domain and communicates only by
publishing/subscribing typed messages on the bus**. No module holds a handle to
another module. The bus is the only shared surface; the message enum is the only
contract. This buys exactly what the user ranks highest: **hot-swappable,
individually-testable modules behind a stable message contract**, with isolated
domains and trivial upgrades.

This document is a build plan, not a survey. It is opinionated where it needs to
be.

---

## 0. The one decision everything hangs on

> **One OS process. One async runtime (Tokio). One typed event bus. N modules,
> each a `tokio::task`, each owning exactly one domain, each touching the bus and
> nothing else.**

We deliberately reject a multi-process / D-Bus / socket-microservice split for
the *internal* architecture. Here is why that is the correct call for *this*
tool, under *this* philosophy:

- The hard isolation we need is **domain isolation**, not **fault isolation
  across address spaces**. A typed Rust enum on an in-process bus already gives a
  stronger, *compile-checked* contract than any IPC schema would. If a module
  panics, we want to know and restart it (supervision, below) — we do not need a
  separate kernel-scheduled process to get that.
- The tool is a single-user, single-session, couch-side companion. It is not a
  fleet. Multiprocess IPC would add serialization, socket lifecycle, and version-
  skew problems **to solve a problem we don't have**, while *weakening* the
  contract from "typed enum" to "bytes on a wire."
- The genuinely-external things (native Discord, gamescope, Steam) are *already*
  separate processes we talk to over their *own* established IPC (the
  `discord-ipc-0` Unix socket; the X11 server; uinput/evdev). Those boundaries
  are real and we honor them — inside an adapter module. We don't invent new ones.

So: **the bus is internal and typed; the IPC is only at the edges, hidden behind
adapter modules.** This is the cleanest possible expression of "isolated domains,
easy upgrades."

---

## 1. SYSTEM DESIGN

### 1.1 Process & deployment model

A single binary, `couchcordd` (the daemon), plus a tiny ancillary binary
`couchcord-overlay` *only if* we choose process-separated rendering (see 1.4 —
we default to in-process and keep the split as a documented fallback).

```
┌─────────────────────────────────────────────────────────────────────┐
│ couchcordd  (one process, Tokio multi-thread runtime)                │
│                                                                       │
│   ┌────────────┐   publishes/   ┌──────────────────────────────┐     │
│   │  Modules    │──subscribes──▶ │  EventBus<Msg>  (tokio bcast │     │
│   │  (tasks)    │◀──────────────│   + per-module mpsc inboxes)  │     │
│   └────────────┘                └──────────────────────────────┘     │
│                                                                       │
│   input-source  ·  discord-rpc  ·  render-sink  ·  menu-fsm           │
│   ·  config  ·  supervisor  ·  (overlay-ipc shim, fallback only)      │
└─────────────────────────────────────────────────────────────────────┘
        │ uinput/evdev        │ discord-ipc-0       │ X11 / gamescope
        ▼                     ▼                     ▼
   virtual kbd from      native Discord        gamescope external
   Steam Input           (background)          overlay window
```

**Lifecycle & supervision — systemd user service, *not* wrapper-spawned.**

We run `couchcordd` as a **systemd *user* service** (`~/.config/systemd/user/
couchcordd.service`), `WantedBy=graphical-session.target`, started when the
graphical/game-mode session comes up.

Rationale, decisively:

- **Criterion 1 (background runtime, never a Steam shortcut, never in the
  focus stack)** is *structurally* satisfied if the daemon is owned by systemd
  and not by Steam. A wrapper/shortcut spawn would make it a child of the Steam/
  gamescope process tree — exactly the focus-trap lineage the spec exists to
  avoid. systemd-owned = lives entirely outside Steam's notion of "games."
- systemd gives us **free supervision at the process level**: `Restart=on-
  failure`, `WatchdogSec=` (the daemon pings `sd_notify` WATCHDOG), startup
  ordering, and journald logging. We get crash-recovery and observability without
  writing a supervisor of our own *for the process*.
- Environment capture: the unit imports `DISPLAY`, `XDG_RUNTIME_DIR`,
  `WAYLAND_DISPLAY`, and the gamescope X display via
  `systemctl --user import-environment` from the session, so the X11/overlay and
  the `discord-ipc-0` socket paths resolve.

```ini
# couchcordd.service  (sketch)
[Unit]
Description=couchcord controller Discord voice control
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=notify                     # sd_notify READY=1 after bus + modules up
ExecStart=%h/.local/bin/couchcordd
Restart=on-failure
WatchdogSec=30
NotifyAccess=main

[Install]
WantedBy=graphical-session.target
```

**Two layers of supervision, on purpose:**

1. **Process layer = systemd.** If the whole daemon dies, systemd restarts it.
2. **Module layer = our in-process `supervisor` module.** If one *task*
   (e.g. the discord-rpc adapter) returns `Err` or panics, the supervisor
   restarts *that module's task* with backoff and publishes a
   `Msg::ModuleHealth` event — the rest of the system keeps running. A flaky
   Discord socket must never take down the input grab or the overlay.

This is the heart of "isolated domains": a failing domain is restarted in place,
behind the same bus contract, without disturbing its neighbors.

### 1.2 IPC / communication

- **Internal:** the typed event bus only (section 2.2). Zero internal sockets.
- **External, behind adapters:**
  - `input-source` ↔ **uinput/evdev**: reads the *virtual* keyboard device that
    Steam Input emits. Never opens the physical controller, never `EVIOCGRAB`s it
    — sidestepping the mid-game exclusivity problem the spec calls out.
  - `discord-rpc` ↔ **`$XDG_RUNTIME_DIR/discord-ipc-0`**: the official Discord
    IPC framing (4-byte LE opcode, 4-byte LE length, JSON body). OAuth
    `AUTHORIZE`→token→`AUTHENTICATE`, then commands + `SUBSCRIBE` events.
  - `render-sink` ↔ **X11 / gamescope**: creates a borderless override-redirect
    window, sets the `GAMESCOPE_EXTERNAL_OVERLAY` atom so gamescope composites it
    on top without putting it in the focus stack (the `discover-overlay`
    technique, reimplemented in Rust).

### 1.3 The four named pieces, placed

| Spec piece                         | Where it lives                                  | Boundary |
|------------------------------------|-------------------------------------------------|----------|
| Background Discord client          | **External process.** We never manage its UI; we connect to its socket from the `discord-rpc` adapter. Optionally a oneshot `couchcord-discord-launch` unit ensures it's backgrounded. | OS process + Unix socket |
| gamescope external-overlay render  | `render-sink` module (X11 + overlay atom)       | bus ⇄ X11 |
| Steam-Input virtual keyboard input | `input-source` module (uinput/evdev reader)     | bus ⇄ evdev |
| Discord RPC connection             | `discord-rpc` module                            | bus ⇄ discord-ipc-0 |

The **menu/state-machine** (`menu-fsm`) is the only module with *no* external
edge: it is pure logic over the bus. That is intentional and makes it trivially
unit-testable (feed it input events, assert the commands/render intents it emits).

### 1.4 Renderer: in-process by default, process-split as a *documented swap*

Default: `render-sink` runs in-process as a task. It owns its X11 connection on a
dedicated thread (X11 isn't `Send`-friendly across awaits, so the adapter runs a
classic blocking event loop on its own thread and bridges to the bus via an mpsc
channel — the rest of the daemon stays pure async).

Because the renderer talks **only** through the bus, swapping it for a
**separate `couchcord-overlay` process** later is a contained change: replace the
in-process `render-sink` with an `overlay-ipc` shim module that serializes the
exact same `RenderIntent` messages over a Unix socket to the external renderer.
**No other module changes**, because they only ever published `RenderIntent` to
the bus. We ship the in-process version; the split is pre-designed insurance.

---

## 2. SOFTWARE DESIGN

### 2.1 Crate / module breakdown (Cargo workspace)

A workspace makes the boundaries *physical* — a module literally cannot reach
into another's internals because it doesn't depend on its crate; everyone depends
only on the contract crate.

```
couchcord/
├─ Cargo.toml                      # [workspace]
├─ crates/
│  ├─ cc-contract/                 # THE message enum + bus trait + shared types.
│  │                               # Depends on NOTHING else in the workspace.
│  ├─ cc-bus/                      # The EventBus implementation (impl of cc-contract::Bus).
│  ├─ cc-input/                    # input-source adapter   (uinput/evdev)
│  ├─ cc-discord/                  # discord-rpc adapter     (discord-ipc-0)
│  ├─ cc-render/                   # render-sink adapter     (X11 + overlay atom)
│  ├─ cc-menu/                     # menu-fsm (pure logic, no external edge)
│  ├─ cc-config/                   # config load/watch, publishes ConfigChanged
│  ├─ cc-supervisor/              # module spawn/restart/health
│  └─ cc-daemon/                   # the couchcordd binary: wires modules to bus, sd_notify
└─ docs/panel/propose-event-driven-core.md
```

**Single responsibility, per crate:**

- **`cc-contract`** — owns *only* the `Msg` enum, the value types it carries
  (`GuildSummary`, `VoiceChannel`, `VoiceMember`, `Anchor`, `RenderIntent`,
  `MenuView`…), and the `Bus`/`Module` traits. This crate is the contract. It has
  no logic and (almost) no dependencies. **Every cross-module change is a change
  *here*, reviewed as a contract change.** This is the linchpin of "easy
  upgrades": the diff that matters is always visible in one small crate.
- **`cc-bus`** — the broadcast/inbox machinery. Pure plumbing. Swappable (e.g.
  swap `tokio::broadcast` for a priority queue) without touching modules.
- **`cc-input`** — turns raw evdev key events from the Steam-Input virtual
  keyboard into semantic `InputEvent`s (`OpenChord`, `Up/Down/Left/Right`,
  `Confirm`, `Back`, `Dismiss`). Owns key→intent mapping and chord debouncing.
  **Knows nothing about Discord or rendering.**
- **`cc-discord`** — owns the IPC socket, OAuth handshake, the confirmed command
  set, subscriptions, and reconnect. Translates bus `DiscordCommand`s → RPC
  frames, and RPC events/responses → bus `DiscordEvent`s. **Knows nothing about
  input, menus, or pixels.**
- **`cc-render`** — owns the overlay window + drawing. Consumes `RenderIntent`
  (a declarative scene), paints it, and handles 8-position anchoring geometry.
  **Knows nothing about why a scene looks the way it does.**
- **`cc-menu`** — the state machine. Consumes `InputEvent` + `DiscordEvent`,
  emits `DiscordCommand` + `RenderIntent` + `InputControl` (grab/release).
  **The only module that understands "what the app does"; the only one with no
  syscalls.**
- **`cc-config`** — loads `client_id`, key map, anchor default, theme; watches
  the file; emits `ConfigChanged`.
- **`cc-supervisor`** — spawns each module's task, catches exit/panic, restarts
  with backoff, emits `ModuleHealth`.

### 2.2 The contract — bus + message enum (the stable spine)

The whole design's stability rests on these signatures. They live in
`cc-contract`.

```rust
// ---- The bus abstraction. Modules depend on this trait, never on cc-bus. ----
#[async_trait::async_trait]
pub trait Bus: Send + Sync + 'static {
    /// Fire-and-forget publish to all subscribers.
    fn publish(&self, msg: Msg);
    /// A filtered subscription. Each module gets its own inbox.
    fn subscribe(&self) -> BusRx;
}

pub struct BusRx { /* wraps tokio::sync::broadcast::Receiver<Msg> */ }
impl BusRx { pub async fn recv(&mut self) -> Result<Msg, BusLagged>; }

// ---- Every module implements this. The supervisor only knows this trait. ----
#[async_trait::async_trait]
pub trait Module: Send + 'static {
    /// Stable identity for health/logging.
    fn id(&self) -> ModuleId;
    /// Run until cancelled or error. Gets a publish handle + its own inbox.
    async fn run(self: Box<Self>, bus: Arc<dyn Bus>, cancel: CancelToken)
        -> anyhow::Result<()>;
}
```

```rust
// ---- THE message contract. One enum. Versioned. The single upgrade surface. --
#[non_exhaustive]                 // adding a variant is never a breaking change
#[derive(Clone, Debug)]
pub enum Msg {
    // --- input domain (produced by cc-input, consumed by cc-menu) ---
    Input(InputEvent),
    // --- input control (produced by cc-menu, consumed by cc-input) ---
    InputControl(InputControl),   // Grab / Release navigation

    // --- discord domain ---
    DiscordCommand(DiscordCommand),   // menu -> discord adapter
    DiscordEvent(DiscordEvent),       // discord adapter -> menu

    // --- render domain ---
    Render(RenderIntent),             // menu -> render-sink (declarative scene)
    RenderAck(RenderStatus),          // render-sink -> bus (e.g. shown/hidden)

    // --- cross-cutting ---
    ConfigChanged(Config),
    ModuleHealth { id: ModuleId, state: HealthState },
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum InputEvent {
    OpenChord, Up, Down, Left, Right, Confirm, Back, Dismiss,
    AnchorCycle(Rotation),   // e.g. a bumper to rotate the overlay anchor
}

#[derive(Clone, Debug)]
pub enum InputControl { GrabNavigation, ReleaseNavigation }

// The Discord boundary is expressed in *domain* terms, NOT raw RPC verbs,
// so the RPC wire format can change underneath without touching the menu.
#[derive(Clone, Debug)]
pub enum DiscordCommand {
    Connect,
    ListGuilds,
    ListVoiceChannels { guild_id: GuildId },
    JoinVoice { channel_id: ChannelId },
    LeaveVoice,
    SubscribeVoiceActivity { channel_id: ChannelId },
    UnsubscribeVoiceActivity { channel_id: ChannelId },
}

#[derive(Clone, Debug)]
pub enum DiscordEvent {
    Connected { user: UserSummary },
    Disconnected { reason: String },
    Guilds(Vec<GuildSummary>),
    VoiceChannels { guild_id: GuildId, channels: Vec<VoiceChannel> },
    JoinedVoice { channel: VoiceChannel },
    LeftVoice,
    VoiceMembers { channel_id: ChannelId, members: Vec<VoiceMember> },
    SpeakingChanged { channel_id: ChannelId, user_id: UserId, speaking: bool },
    Error { command: &'static str, message: String },
}

// Declarative scene. The menu describes WHAT to show; render decides pixels.
#[derive(Clone, Debug)]
pub enum RenderIntent {
    Hidden,
    Menu(MenuView),                       // full Steam-styled menu scene
    Overlay { anchor: Anchor, roster: Vec<VoiceMember> },  // activity HUD
    MenuWithOverlay { menu: MenuView, anchor: Anchor, roster: Vec<VoiceMember> },
}

#[derive(Clone, Copy, Debug)]
pub enum Anchor {                          // criterion 7: exactly 8 positions
    TopLeft, TopCenter, TopRight,
    MidLeft,             MidRight,
    BottomLeft, BottomCenter, BottomRight,
}
```

**Why domain-level Discord messages, not raw RPC verbs, is the most important
single choice in 2.2:** the menu speaks `JoinVoice { channel_id }`, never
`SELECT_VOICE_CHANNEL`. If Discord changes the verb, adds a field, or we someday
proxy through a bot, **only `cc-discord` changes**; the contract and every other
module are untouched. The blast radius of a Discord change is *one crate*.

### 2.3 Module run-loop shape (uniform, testable)

Every module is the same shape: a `select!` over its inbox + its external edge.

```rust
// cc-menu (sketch) — pure: no syscalls, fully unit-testable.
async fn run(self: Box<Self>, bus: Arc<dyn Bus>, cancel: CancelToken)
    -> anyhow::Result<()>
{
    let mut rx = bus.subscribe();
    let mut sm = MenuStateMachine::new(self.config);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            msg = rx.recv() => match msg? {
                Msg::Input(ev)        => sm.on_input(ev,    &bus),
                Msg::DiscordEvent(ev) => sm.on_discord(ev,  &bus),
                Msg::ConfigChanged(c) => sm.set_config(c),
                _ => {}
            }
        }
    }
    Ok(())
}
```

`MenuStateMachine` emits via `bus.publish(...)`. In tests we hand it a
`MockBus` that records publishes, drive `on_input`/`on_discord` directly, and
assert the emitted `DiscordCommand`/`RenderIntent` sequence. No Tokio, no
sockets, no X server. That is the "individually testable" guarantee, made
concrete.

### 2.4 The menu state machine

```rust
enum MenuState {
    Closed,
    GuildList   { guilds: Vec<GuildSummary>, cursor: usize },
    ChannelList { guild: GuildId, channels: Vec<VoiceChannel>, cursor: usize },
    Connected   { channel: VoiceChannel },   // overlay live; menu may be closed
}
```

Transitions are a pure function `(MenuState, Event) -> (MenuState, Vec<Msg>)`.
The overlay (`Connected` roster) is tracked independently of menu open/closed, so
"who's talking" keeps rendering after the menu dismisses.

### 2.5 Data/event flow for each required interaction

Legend: `cc-input → BUS → cc-menu → BUS → {cc-discord, cc-render}`.

**Open menu (chord):**
1. Steam Input action-layer emits the signal key → virtual kbd.
2. `cc-input` reads it → `Msg::Input(OpenChord)`.
3. `cc-menu`: `Closed → GuildList(loading)`. Publishes
   `Msg::InputControl(GrabNavigation)` (input now routes nav keys, criterion 3),
   `Msg::DiscordCommand(ListGuilds)`, and `Msg::Render(Menu(loading view))`.
4. `cc-render` paints the Steam-styled menu; `cc-discord` issues `GET_GUILDS`.

**Browse servers:**
1. `cc-input`: d-pad → `Up/Down` events.
2. `cc-menu`: moves `cursor`, emits `Render(Menu(updated))`. (No Discord traffic;
   list already cached from `DiscordEvent::Guilds`.)

**List voice channels:**
1. `cc-input`: A → `Confirm`.
2. `cc-menu` (`GuildList`): emits `DiscordCommand(ListVoiceChannels{guild})`,
   transitions to `ChannelList(loading)`, emits `Render`.
3. `cc-discord`: `GET_CHANNELS` for guild → filters `type==2` (and optional
   `13`) → `DiscordEvent(VoiceChannels{..})`.
4. `cc-menu`: fills list, `Render(Menu(channels))`.

**Select voice channel:**
1. `Confirm` in `ChannelList` → `cc-menu` emits
   `DiscordCommand(JoinVoice{channel_id})` and
   `DiscordCommand(SubscribeVoiceActivity{channel_id})`.
2. `cc-discord`: `SELECT_VOICE_CHANNEL` + `SUBSCRIBE` voice-state/speaking.
3. On `DiscordEvent(JoinedVoice)` → `cc-menu`: state `Connected`, emits
   `Render(MenuWithOverlay)` (or `Render(Overlay)` if user dismisses menu).

**Leave:**
1. `Confirm` on the "Leave" affordance (or a dedicated `Back` semantics in
   `Connected`) → `cc-menu` emits `DiscordCommand(LeaveVoice)` +
   `UnsubscribeVoiceActivity`.
2. `cc-discord`: `SELECT_VOICE_CHANNEL{channel_id:null}` → `DiscordEvent(LeftVoice)`.
3. `cc-menu`: `Connected → ChannelList`, `Render(Menu)`, overlay roster cleared.

**Render voice activity (live overlay):**
1. While `Connected`, `cc-discord` receives `SPEAKING_START/STOP` + voice-state
   events → `DiscordEvent(SpeakingChanged)` / `VoiceMembers`.
2. `cc-menu` updates roster, emits `Render(Overlay{anchor, roster})`.
3. `cc-render` repaints only the overlay (speaking = highlighted ring). This
   continues with the menu closed and input *released* back to the game.

**Reposition overlay (8 positions):**
1. A bound control (bumper) → `cc-input` → `Msg::Input(AnchorCycle(Next))`.
2. `cc-menu` advances `Anchor` through the 8-variant enum, emits
   `Render(Overlay{new anchor, roster})` and `ConfigChanged` persistence intent.
3. `cc-render` recomputes window geometry for the anchor and repaints. Anchor
   geometry math lives entirely in `cc-render`; the menu only names the position.

**Dismiss:**
- `Dismiss`/`Back`-at-root → `cc-menu` emits `InputControl(ReleaseNavigation)`
  (criterion 3: focus returns to game) and `Render(Overlay or Hidden)`.

---

## 3. HOW IT MEETS EACH LOCKED SUCCESS CRITERION

1. **Background runtime, never a focus-stack surface.** `couchcordd` is a
   **systemd user service**, outside the Steam/gamescope process tree entirely.
   It is not a shortcut, not a "game." The overlay window carries
   `GAMESCOPE_EXTERNAL_OVERLAY`, so gamescope composites it on top *without* a
   focus-stack entry. Exiting a game can never land on us. ✔ (structural, by
   process ownership + overlay atom)
2. **Local official RPC only.** All Discord I/O is confined to `cc-discord`,
   which speaks *only* the `discord-ipc-0` socket using the confirmed command
   set (`AUTHORIZE`/`AUTHENTICATE`/`GET_GUILDS`/`GET_CHANNELS`/
   `SELECT_VOICE_CHANNEL`/`GET_SELECTED_VOICE_CHANNEL`/`SUBSCRIBE`…). No other
   crate can even reach the socket. The user's `client_id` comes from
   `cc-config`. ✔
3. **Chord opens the GUI, grabs input, releases on dismiss.** `cc-input` detects
   `OpenChord`; `cc-menu` emits `GrabNavigation`; on dismiss it emits
   `ReleaseNavigation`. Because we read the *virtual* Steam-Input keyboard (never
   `EVIOCGRAB` the physical pad), the grab is cooperative and mid-game-safe. ✔
4. **Steam-client-styled GUI over the active window.** `cc-render` owns a Steam-
   styled theme (`cc-config` supplies palette/spacing) and draws into the
   external-overlay window that composites over whatever's running. The menu
   scene is fully described by `MenuView`. ✔
5. **Server → voice-channel browser/selector, voice-only.** The
   `GuildList → ChannelList` flow above; `cc-discord` filters `type==2` (+ opt
   `13`) so non-voice channels never reach the menu. ✔
6. **Leave channel.** `DiscordCommand::LeaveVoice` →
   `SELECT_VOICE_CHANNEL{null}`. ✔
7. **Voice-activity overlay, 8 anchors.** `RenderIntent::Overlay{anchor,roster}`
   driven by `SUBSCRIBE`d speaking/voice-state events; `Anchor` is an 8-variant
   enum (4 corners + 4 edge midpoints); `AnchorCycle` rotates it; geometry in
   `cc-render`. ✔
   (Soundboard correctly absent — no contract variant for it; nothing to build.)

---

## 4. UPGRADE & ISOLATION (the user's top priority)

**The invariant that makes all of this true:** modules never reference each
other; they reference only `cc-contract`. Therefore the *only* way to create
coupling is to change the message enum — which is a visible, reviewable,
single-crate event. Everything below is a consequence of that invariant.

### 4.1 Swapping / upgrading each domain

- **Discord RPC change** (Discord adds a field, renames a verb, changes framing,
  or we move to a proxying bot): edit **`cc-discord` only**. The menu speaks
  domain verbs (`JoinVoice`), not RPC verbs, so it is untouched. *Blast radius:
  one crate, behind the same `DiscordCommand`/`DiscordEvent` contract.* If the
  change requires *new capability* (e.g. a new event), you add a `#[non_
  exhaustive]` variant to `DiscordEvent` — additive, non-breaking; consumers that
  don't care ignore it.
- **Renderer swap** (X11 overlay → a Wayland layer-shell renderer, or in-process
  → external `couchcord-overlay` process): replace **`cc-render`** with a new
  impl of the same `RenderIntent` consumer (or insert the `overlay-ipc` shim).
  The menu still just publishes `RenderIntent`. *Blast radius: one crate.* The
  process-split fallback is literally a drop-in because the contract is already
  message-shaped.
- **Input-method change** (Steam-Input virtual keyboard → raw evdev with a
  different grab strategy, or a network remote, or a test harness): replace
  **`cc-input`** with anything that emits `Msg::Input(..)` and honors
  `InputControl`. *Blast radius: one crate.* The menu never knew where keys came
  from.
- **Menu/UX change** (new flows, reordering, new affordances): edit **`cc-menu`**
  only — it's pure logic and has the richest test suite, so this is the safest
  crate to iterate on. No adapter changes unless a genuinely new capability is
  needed (then: add one contract variant + implement it in one adapter).
- **Bus implementation change** (broadcast → priority queue, add tracing/replay):
  edit **`cc-bus`** only; the `Bus` trait is the seam.

### 4.2 Blast-radius table (explicit)

| Change                              | Crates touched            | Contract change? | Other modules rebuilt? |
|-------------------------------------|---------------------------|------------------|------------------------|
| Discord adds field to channel       | `cc-discord`              | no (internal)    | no                     |
| New Discord event we want to use    | `cc-discord` + 1 consumer | additive variant | only the consumer      |
| X11 overlay → Wayland layer-shell   | `cc-render`               | no               | no                     |
| In-process render → separate process| `cc-render`→`overlay-ipc` | no               | no                     |
| Steam-Input kbd → raw evdev         | `cc-input`                | no               | no                     |
| New menu flow / restyle             | `cc-menu` (+`cc-config`)  | no               | no                     |
| Add a 9th... no — anchors are fixed | n/a                       | n/a              | n/a                    |

The "Other modules rebuilt? = no" column **is** the deliverable the user asked
for. Isolation is enforced by the dependency graph (workspace), not by
discipline.

### 4.3 Versioning & live validation

- `cc-contract` carries a `CONTRACT_VERSION`. The daemon logs it at boot. Because
  the two spec items needing live validation (`SELECT_VOICE_CHANNEL` end-to-end;
  Steam-Input flow reaching the daemon) are each **confined to one adapter**, we
  validate them in isolation: run `cc-discord` with a stub bus printing events to
  confirm the RPC path; run `cc-input` with a stub bus to confirm virtual-kbd
  keys arrive — *before* wiring the full app. The architecture makes the risky
  items independently testable, which is exactly how you de-risk a build.

---

## 5. THE 3 BIGGEST HONEST RISKS / WEAKNESSES OF THIS APPROACH

1. **Broadcast-bus fan-out and lag are real failure modes.** A single
   `tokio::broadcast` channel delivers every `Msg` to every subscriber. High-rate
   `SpeakingChanged` events fan out to modules that don't care, and a slow
   consumer (the X11 render thread blocked on a repaint) can lag and *drop*
   messages (`broadcast::error::Lagged`). If `cc-render` lags and drops a
   `JoinedVoice`, the overlay state silently desyncs. **Mitigation we commit to:**
   per-module bounded inboxes with explicit overflow policy (coalesce render
   intents — only the latest scene matters; never coalesce `DiscordCommand`s);
   treat `Lagged` as a module-health event that triggers a state resync request,
   not a silent drop. But this is genuine added complexity the "small bus" framing
   can hide, and it must be designed, not assumed.

2. **The typed enum is a hard coupling point precisely *because* it's the
   contract.** Every module compiles against the *whole* `Msg` enum. A change that
   *is* breaking (renaming a field used by three modules, changing a payload
   type) recompiles and can break multiple crates at once — the very blast radius
   we're proud of avoiding. `#[non_exhaustive]` + additive variants keep most
   changes cheap, but **structural changes to existing payloads are not isolated**,
   and there's a temptation to dump unrelated concerns into one fat enum. We
   accept this: domain-typed sub-enums (`DiscordCommand`, `InputEvent`) localize
   most churn, and "changing the contract is a real event" is a *feature* for a
   modularity-first project — but it is the honest cost of one shared spine.

3. **In-process modularity gives weaker *fault* isolation than the architecture's
   tidiness implies.** Domains are isolated *logically*, but they share one
   address space and one Tokio runtime. A panic in `cc-render`'s X11 thread, an
   FFI misuse in uinput, or an unbounded memory grow in any module can take down
   the whole daemon despite the clean boundaries — systemd restarts the process,
   but that's a full reset, not the graceful per-module restart we advertise. The
   in-process `supervisor` catches task `Err`/panic *unwinds*, but cannot save us
   from `abort`-on-panic in FFI or a deadlocked blocking thread. The
   pre-designed `overlay-ipc` process split is our pressure valve for the single
   riskiest FFI boundary (X11), but until we pull it, "isolated domains" is a
   compile-time and contract-time truth more than a runtime-fault truth — and we
   should say so plainly rather than oversell it.

---

## Appendix A — Build order (so this is actionable)

1. `cc-contract` (the enum + traits) — freeze v0 of the contract first.
2. `cc-bus` + `cc-supervisor` + `cc-daemon` skeleton with `sd_notify`.
3. `cc-discord` against a stub bus → **live-validate `SELECT_VOICE_CHANNEL`.**
4. `cc-input` against a stub bus → **live-validate Steam-Input virtual kbd.**
5. `cc-menu` (pure, TDD) — full flow coverage with `MockBus`.
6. `cc-render` (X11 overlay + 8-anchor geometry + Steam theme).
7. Wire all under the daemon; ship `couchcordd.service` + Steam Input template.
8. (Insurance, deferred) `overlay-ipc` external-renderer split.

## Appendix B — Why not multiprocess/D-Bus internally (one line)

A typed Rust enum on an in-process bus is a *stronger, compile-checked* contract
than any serialized IPC, and this tool is one single-user session — so we spend
our isolation budget on **domain boundaries + supervision**, not on address-space
splits we don't need. The only IPC we keep is the IPC that already exists at the
real external edges (Discord socket, X11, uinput).
