# couchcord — Architecture Proposal: Multi-Process Services

**Philosophy (assigned, exclusive):** Separate long-lived services, one per domain,
each its own OS process, communicating over a local IPC (unix socket + a small
typed message protocol). Optimize for **independent upgrade/restart** and
**hard process-level domain isolation**.

**User's weighted-highest priority:** highly modular software, clear
service/module boundaries, easy upgrades, isolated domains. This outranks raw
simplicity. Every decision below is made to maximize that, *within* the
multi-process philosophy.

---

## 0. TL;DR of the design

Four long-lived daemons plus one supervisor, each a separate `systemd --user`
service, wired together by a single in-process **message bus daemon**
(`couchd`, the orchestrator) over per-service unix sockets carrying
length-prefixed CBOR frames of one versioned `Envelope` enum. Domains never talk
to each other directly — they publish/subscribe through the orchestrator. Any
domain process can be `systemctl --user restart`ed in isolation; the orchestrator
buffers and re-handshakes it. The four domains map 1:1 to the four hard problems
in the spec: **Discord RPC**, **overlay rendering**, **controller/keyboard
input**, **UI/menu state**.

```
                       ┌─────────────────────────────────────────┐
                       │   couchd  (orchestrator / message bus)   │
                       │   owns: routing, lifecycle, UI state FSM │
                       └───┬──────────┬───────────┬───────────┬───┘
        unix socket /run/  │          │           │           │
        user/$UID/couchcord/  │       │           │           │
            ├ rpc.sock ◀──────┘       │           │           │
            ├ input.sock ◀────────────┘           │           │
            ├ render.sock ◀───────────────────────┘           │
            └ (couchd is the server; daemons dial in) ◀───────┘
   ┌────────────────┐  ┌────────────────┐  ┌────────────────┐
   │ couch-rpcd     │  │ couch-inputd   │  │ couch-renderd  │
   │ Discord IPC    │  │ uinput vkbd    │  │ gamescope X11  │
   │ discord-ipc-0  │  │ reader/grabber │  │ external overlay│
   └───────┬────────┘  └───────┬────────┘  └───────┬────────┘
   discord-ipc-0          /dev/input/eventN     X11 / GAMESCOPE_
   (Discord client)       (Steam virtual kbd)   EXTERNAL_OVERLAY
```

---

## 1. SYSTEM DESIGN

### 1.1 Processes / services (the domain decomposition)

| Process | Domain (single responsibility) | Owns / touches | Crash blast radius |
|---|---|---|---|
| **`couchd`** | Orchestration: message routing, service registry, UI state machine, position model | The bus socket; the canonical menu/overlay state | Whole app pauses, but no game impact; restart re-handshakes children |
| **`couch-rpcd`** | Discord RPC bridge | `discord-ipc-0` socket, OAuth token, guild/channel/voice-state cache | Voice control + activity data stop; game + overlay frame survive |
| **`couch-renderd`** | Overlay rendering | gamescope external-overlay X11 window, GPU surface, fonts | Overlay disappears; voice + input keep working headless |
| **`couch-inputd`** | Input mediation | uinput virtual-kbd evdev device, grab/ungrab, chord detection | Chord/nav dead; everything already-open stays; game input unaffected |

Why exactly these four: they are the four spec domains with **disjoint external
resources** (a Discord socket, an X11/GPU surface, an evdev device) and **disjoint
failure modes**. Splitting finer (e.g. a separate "voice-state cache" process)
would add IPC cost without isolating a distinct external resource. Splitting
coarser violates the philosophy and re-couples the very domains the user wants
isolated. Four is the natural cut.

`couchd` is deliberately the **only** stateful coordinator. The three domain
daemons are as close to stateless translators of their external resource as
possible (rpcd holds a cache, but it is rebuildable from Discord on reconnect).
This is what makes upgrades cheap: a domain daemon can be killed and replaced and
`couchd` rebuilds the world.

### 1.2 Lifecycle & supervision — **systemd --user**, not wrapper-spawned

Decision: **`systemd --user` with a target unit**, one service unit per process.
Rationale tied to the priority:

- **Independent upgrade/restart is a first-class systemd verb.** `systemctl
  --user restart couch-renderd` swaps the renderer with zero custom supervision
  code. A hand-rolled wrapper-spawner would re-implement restart, backoff,
  socket-activation, and crash policy — badly.
- **Hard isolation** is enforced by systemd: per-service `MemoryMax`,
  `Restart=on-failure`, `RestartSec`, and (crucially) the **socket-activation**
  model decouples "is the socket up" from "is the process up".
- It survives the game-mode session correctly: these are user services, not tied
  to any Steam shortcut → criterion 1 (never a focus-stack surface) is structural.

Unit layout (`~/.config/systemd/user/`):

```
couchcord.target              # the thing you enable; wants the 4 below
couchd.service                # orchestrator; Type=notify (sd_notify READY)
couch-rpcd.service            # After=couchd.service, BindsTo=couchd.service
couch-renderd.service         # After=couchd.service
couch-inputd.service          # After=couchd.service
couchd.socket                 # socket-activated bus socket (systemd owns the fd)
```

Key directives:

- `couchd.socket` holds `ListenStream=%t/couchcord/bus.sock`. systemd creates and
  owns the listening socket; `couchd` receives it via `LISTEN_FDS`. **This is the
  upgrade superpower:** restarting `couchd` does **not** drop the socket, so
  domain daemons' connections survive a reconnect loop without a thundering-herd
  of "connection refused".
- Domain services use `Restart=on-failure`, `RestartSec=500ms`,
  `StartLimitBurst=5`. A crash-looping domain is quarantined by systemd, not by
  app code.
- `BindsTo=couchd.service` on the three domains: if the orchestrator is
  intentionally stopped, the domains stop too (clean shutdown). But `After=` only
  for restart ordering, so a *crash* of couchd lets domains keep their external
  resources and just reconnect.
- `couch-inputd` and `couch-renderd` get the X11/Wayland env (`DISPLAY`,
  `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`) imported from the session via
  `systemctl --user import-environment` in the session startup, or a drop-in.
- Sandboxing per unit (free isolation): `couch-rpcd` gets
  `RestrictAddressFamilies=AF_UNIX` (it only needs the Discord local socket),
  no network namespace beyond that; `couch-renderd` keeps GPU device access;
  `couch-inputd` is the only unit with `DeviceAllow=/dev/uinput rw` and
  `SupplementaryGroups=input`. Each domain's *capabilities* are scoped to its
  domain — defense in depth that doubles as documentation of the boundary.

### 1.3 Deployment

- A single Cargo **workspace** (see §2) builds five binaries into one prefix.
- Install target: `~/.local/bin/couch{d,-rpcd,-renderd,-inputd}` + unit files +
  the Steam Input controller template + an example `client_id` config.
- `couchcordctl` (thin CLI, ships in the workspace) does:
  `couchcordctl install` (writes units, `systemctl --user daemon-reload`,
  `enable --now couchcord.target`), `couchcordctl status`, `couchcordctl logs`.
- Config: one TOML at `~/.config/couchcord/config.toml` (client_id, default
  overlay position, key bindings). Each daemon reads only its own `[section]` —
  no shared mutable config object, no cross-domain config coupling.

### 1.4 IPC / communication protocol

**Transport:** unix `SOCK_STREAM`, one connection per domain daemon, all dialing
the single `couchd` bus socket. `couchd` is the server (star topology). Domains
**never** open sockets to each other — enforced structurally by only ever giving
them the bus path. This is the core of "isolated domains": the only thing a
domain can break is its own connection to the bus.

**Framing:** 4-byte big-endian length prefix + **CBOR** body. CBOR (not JSON)
because: compact, schema-evolution-friendly (unknown fields skipped), and serde
gives it for free. One wire type only: the `Envelope`.

**Message model — pub/sub + request/response over one enum:**

```rust
// crate: couch-proto  (the ONLY crate every process shares)

/// Stable wire identity of a service. Adding a service = adding a variant.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceId { Orchestrator, Rpc, Render, Input }

/// Every frame on every socket is one of these.
#[derive(Serialize, Deserialize)]
pub enum Envelope {
    /// First frame a daemon sends after connecting.
    Hello { service: ServiceId, proto: ProtoVersion, build: BuildInfo },
    /// couchd's reply; tells the daemon which event topics it'll receive.
    Welcome { session: SessionId, subscribe: Vec<Topic> },
    /// Fire-and-forget domain event (pub/sub). e.g. SpeakingStarted.
    Event(Event),
    /// Request expecting exactly one Response with matching id (req/rep).
    Request { id: ReqId, body: Command },
    Response { id: ReqId, body: Result<Reply, ProtoError> },
    /// Liveness; couchd pings, daemon pongs. Drives the registry.
    Ping(u64), Pong(u64),
}

pub struct ProtoVersion { pub major: u16, pub minor: u16 }
```

`major`/`minor` proto versioning is the **upgrade contract**: a daemon and
`couchd` negotiate the lowest common `minor` within a matching `major` at
`Hello`/`Welcome`. Adding a backward-compatible message bumps `minor`; a breaking
change bumps `major` and `couchd` refuses the `Hello` (and logs it) rather than
mis-parsing. So you can upgrade `couch-rpcd` independently as long as proto
`major` is unchanged.

**Domain command/event vocabulary** (the typed messages — these *are* the
boundary):

```rust
pub enum Command {
    // → Rpc
    RpcAuthorize,                       // run OAuth/authenticate handshake
    RpcGetGuilds,
    RpcGetVoiceChannels { guild: GuildId },
    RpcSelectVoiceChannel { channel: Option<ChannelId> }, // None = leave
    RpcGetSelectedVoice,
    // → Render
    RenderShow(MenuModel),              // full menu snapshot to draw
    RenderHide,
    RenderOverlay(OverlayModel),        // voice-activity panel
    RenderSetPosition(Anchor),          // one of 8
    // → Input
    InputGrab,                          // start consuming vkbd, enter nav mode
    InputRelease,
}

pub enum Event {
    // from Rpc
    GuildsListed(Vec<Guild>),
    VoiceChannelsListed { guild: GuildId, channels: Vec<VoiceChannel> },
    VoiceStateUpdate { channel: ChannelId, members: Vec<Member> },
    SpeakingStarted { user: UserId }, SpeakingStopped { user: UserId },
    RpcDisconnected,                    // Discord client went away
    // from Input
    Chord(ChordId),                     // the "open menu" chord fired
    Nav(NavKey),                        // Up/Down/Left/Right/Enter/Esc
    // from Render
    RenderReady, RenderLost,            // surface created / X server dropped
    // lifecycle (couchd → subscribers)
    ServiceUp(ServiceId), ServiceDown(ServiceId),
}

pub enum Anchor {
    TopLeft, TopCenter, TopRight,
    MiddleLeft,           MiddleRight,
    BottomLeft, BottomCenter, BottomRight,
}
```

**Delivery semantics:** events are at-most-once, dropped if a subscriber's send
buffer is full (overlay frames are idempotent snapshots, so a dropped one self-
heals on the next). Requests are at-most-once with a timeout; `couchd` returns
`ProtoError::ServiceUnavailable` if the target daemon is down, so the UI degrades
gracefully (e.g. shows "Discord disconnected") instead of hanging.

### 1.5 How the four hard pieces fit

- **Background Discord client:** out-of-scope as a process *we* manage; it's
  launched/backgrounded by the session. `couch-rpcd` *connects to it* at
  `$XDG_RUNTIME_DIR/discord-ipc-0`. If Discord isn't up, `rpcd` sits in a
  reconnect loop and reports `RpcDisconnected` — no other domain notices.
- **gamescope external-overlay renderer:** wholly inside `couch-renderd`. It
  creates the X11 window, sets the `GAMESCOPE_EXTERNAL_OVERLAY` atom, and is the
  *only* process that touches a display surface. Criterion 1 holds because this
  window is an external overlay, never a top-level focus-stack surface.
- **Steam-Input keyboard-mediated input via uinput:** wholly inside
  `couch-inputd`. Steam Input emits a virtual keyboard (signal key + remapped
  nav keys) on a uinput device; `inputd` reads *that* evdev node. It never
  touches the physical pad → sidesteps `EVIOCGRAB`. On `Chord`, `inputd` grabs
  the *virtual keyboard* device so nav keys don't leak to the game, and ungrabs
  on release.
- **Discord RPC connection:** wholly inside `couch-rpcd` — the OAuth token, the
  command set, the SUBSCRIBE streams. The token never leaves this process.

---

## 2. SOFTWARE DESIGN (Rust)

### 2.1 Cargo workspace / crate breakdown

```
couchcord/
├── Cargo.toml                      # [workspace]
├── crates/
│   ├── couch-proto/                # ★ the shared contract. NOTHING else shared.
│   │   └── Envelope, Command, Event, ServiceId, Anchor, versioning, error types
│   ├── couch-ipc/                  # transport: frame codec, async Connection,
│   │   └── reconnect, Hello/Welcome handshake, Ping/Pong. Depends only on proto.
│   ├── couchd/         (bin)       # orchestrator: registry, router, UI FSM
│   ├── couch-rpcd/     (bin)       # Discord domain
│   ├── couch-renderd/  (bin)       # render domain
│   ├── couch-inputd/   (bin)       # input domain
│   └── couchcordctl/   (bin)       # install/status/logs CLI
└── assets/
    ├── systemd/*.service|*.socket|*.target
    └── steam-input/template.vdf
```

**Dependency rule (enforced, the spine of the modularity):**
every binary depends on `couch-ipc` and `couch-proto` and on **nothing else in
the workspace**. No binary depends on another binary's crate. `couch-proto` has
**zero** dependencies on domain logic (no `discord`, no `x11`, no `uinput`
crates). This guarantees a Discord-RPC implementation change cannot force a
recompile-coupling into the renderer: they only share the wire contract.

### 2.2 Boundary + responsibility of each crate

- **`couch-proto`** — *the only coupling point in the system.* Pure data types +
  versioning. Single responsibility: define the wire language. Changing a domain
  must not change this crate unless the *contract* changes (which is exactly the
  event you want to be loud and reviewed).
- **`couch-ipc`** — transport mechanics, domain-agnostic. Single responsibility:
  turn a unix socket into a typed, reconnecting, heartbeated `Connection<Env>`.
  Reused identically by all five binaries.
- **`couchd`** — coordination only. Holds *no* domain knowledge (doesn't know what
  a "voice channel" is beyond passing it through). Owns the UI state machine and
  routing table. SRP: "who is alive, what is the menu showing, where does this
  message go."
- **`couch-rpcd`** — *all* Discord knowledge lives here and nowhere else.
- **`couch-renderd`** — *all* pixel/GPU/X11 knowledge lives here and nowhere else.
- **`couch-inputd`** — *all* evdev/uinput/chord knowledge lives here and nowhere else.

### 2.3 Key public traits / interfaces at each boundary

**Transport boundary (`couch-ipc`):**

```rust
/// A live, framed, auto-reconnecting link to the bus. Generic over nothing —
/// it always speaks Envelope. This is the single chokepoint every process uses.
pub struct Connection { /* ... */ }

impl Connection {
    /// Dial the bus, send Hello, await Welcome. Retries with backoff forever.
    pub async fn connect(bus: &Path, hello: Hello) -> Connection;
    /// Next inbound frame (Event / Request / Response / Ping).
    pub async fn recv(&mut self) -> Result<Envelope, IpcError>;
    pub async fn send(&mut self, env: Envelope) -> Result<(), IpcError>;
    /// Sugar: request/response with timeout + correlation-id bookkeeping.
    pub async fn request(&mut self, c: Command, t: Duration)
        -> Result<Reply, ProtoError>;
}
```

**The domain-service contract (every domain daemon implements this):**

```rust
/// A domain daemon = a translator between its external resource and the bus.
/// couchd never sees this trait; it's the internal shape that makes the three
/// daemons structurally identical and independently swappable.
#[async_trait]
pub trait DomainService {
    const ID: ServiceId;
    /// Handle one inbound Command, optionally producing a Reply.
    async fn on_command(&mut self, cmd: Command) -> Result<Reply, ProtoError>;
    /// Produce outbound Events (driven by the external resource, e.g. a
    /// Discord SUBSCRIBE stream or an evdev read loop). Yields to the bus.
    async fn run_source(&mut self, tx: EventSink) -> !;
}
```

A new input method (say, native evdev instead of Steam Input) is *a new
`DomainService` impl emitting the same `Event::Chord/Nav`*. couchd is untouched.

**Discord boundary (inside `couch-rpcd`, lets you swap RPC impls/mocks):**

```rust
#[async_trait]
pub trait DiscordRpc {
    async fn authenticate(&mut self) -> Result<User, RpcError>;
    async fn guilds(&mut self) -> Result<Vec<Guild>, RpcError>;
    /// Pre-filtered to type==2 (and 13 if enabled). Filter lives here, once.
    async fn voice_channels(&mut self, g: GuildId)
        -> Result<Vec<VoiceChannel>, RpcError>;
    async fn select_voice(&mut self, c: Option<ChannelId>) -> Result<(), RpcError>;
    async fn selected_voice(&mut self) -> Result<Option<ChannelId>, RpcError>;
    /// Long-lived: yields VoiceStateUpdate / Speaking* until disconnect.
    fn subscribe_voice(&mut self, c: ChannelId) -> BoxStream<VoiceEvent>;
}
```

**Render boundary (inside `couch-renderd`, lets you swap the GPU backend):**

```rust
pub trait OverlaySurface {
    /// Create/attach the gamescope external-overlay window.
    fn realize(&mut self) -> Result<(), RenderError>;
    /// Redraw from an immutable snapshot. Snapshots are idempotent → dropped
    /// frames self-heal.
    fn draw(&mut self, frame: &Frame) -> Result<(), RenderError>;
    fn set_anchor(&mut self, a: Anchor, screen: ScreenGeom);
    fn destroy(&mut self);
}

/// Mapping the 8 anchors to pixel rects is pure logic, unit-testable, lives here.
pub fn anchor_rect(a: Anchor, win: Size, screen: ScreenGeom, pad: u32) -> Rect;
```

**Input boundary (inside `couch-inputd`, lets you swap input source):**

```rust
pub trait InputSource {
    /// Stream of decoded key events from the virtual keyboard evdev node.
    fn events(&mut self) -> BoxStream<KeyEvent>;
    /// Grab/ungrab the *virtual* device so nav keys don't reach the game.
    fn grab(&mut self) -> Result<(), InputError>;
    fn ungrab(&mut self);
}
/// Chord + nav recognition is pure logic over KeyEvents → Event. Unit-tested,
/// no I/O. Swapping Steam-Input for raw-evdev only swaps the InputSource impl.
pub struct KeymapFsm { /* ... */ }
```

**Orchestrator UI state machine (inside `couchd`):**

```rust
/// The single source of truth for "what is on screen." Domains are dumb;
/// this is the brain. Pure state → effects; trivially testable.
pub enum UiState {
    Idle,                              // overlay only (if connected), no menu
    ServerList { guilds: Vec<Guild>, sel: usize },
    ChannelList { guild: GuildId, chans: Vec<VoiceChannel>, sel: usize },
    Connected { channel: ChannelId },  // menu dismissed, overlay live
}
pub enum UiEffect {
    To(ServiceId, Command),            // send a command to a domain
    Render(RenderTarget),              // recompute MenuModel/OverlayModel
}
impl UiState {
    /// (next state, effects). The whole nav flow is this one pure function.
    pub fn on(self, ev: &Event) -> (UiState, Vec<UiEffect>);
}
```

### 2.4 Data / event flow for the required interactions

Notation: `I=couch-inputd  D=couchd  R=couch-rpcd  V=couch-renderd`.

**Open menu (chord):**
```
Steam Input → vkbd → I reads signal key → I grabs vkbd
I --Event::Chord--> D
D: UiState::Idle → ServerList (effects: To(Rpc,RpcGetGuilds), Render(Menu))
D --Request RpcGetGuilds--> R --discord GET_GUILDS--> Discord
R --Response GuildsListed--> D  (state fills guilds)
D --Render(MenuModel)--> V  draws Steam-styled server list, grabs focus visually
```

**Browse servers (nav up/down):**
```
I --Event::Nav(Up/Down)--> D
D: ServerList.sel ±= 1 (pure) → effect Render(MenuModel)
D --RenderShow(MenuModel)--> V  redraw highlight. No RPC traffic.
```

**List voice channels (select a server, Enter):**
```
I --Event::Nav(Enter)--> D
D: ServerList → ChannelList (effect To(Rpc, RpcGetVoiceChannels{guild}))
D --Request--> R --GET_CHANNELS, filter type==2/13--> Discord
R --Response VoiceChannelsListed--> D
D --RenderShow(channel list)--> V
```

**Select voice channel (Enter on a channel):**
```
I --Nav(Enter)--> D
D: ChannelList → Connected{channel} (effects:
     To(Rpc, RpcSelectVoiceChannel{Some(ch)}),
     To(Rpc, subscribe voice+speaking),   // rpcd issues SUBSCRIBE
     To(Input, InputRelease),             // hand control back to game
     Render(OverlayModel))
R --SELECT_VOICE_CHANNEL--> Discord (joins)
R starts emitting VoiceStateUpdate/Speaking* events
D --RenderHide(menu)+RenderOverlay--> V   menu closes, activity overlay stays
I ungrabs vkbd → game input flows again
```

**Leave:**
```
(from menu) I --Nav(Enter on "Leave")--> D
D: Connected → Idle (effect To(Rpc, RpcSelectVoiceChannel{None}))
R --SELECT_VOICE_CHANNEL channel_id=null--> Discord (leaves)
R --Event::RpcDisconnected/empty voice state--> D
D --RenderOverlay(empty)/RenderHide--> V  overlay clears
```

**Render voice activity (continuous, no menu):**
```
Discord speaking event → R --Event::SpeakingStarted{user}--> D
D: update Connected member model (pure) → effect Render(OverlayModel)
D --RenderOverlay(OverlayModel)--> V  redraw who's-talking ring/highlight
(Throttled in couchd: coalesce bursts to ≤ N redraws/sec before hitting V.)
```

**Reposition overlay (8 positions):**
```
I --Nav (position chord / menu item)--> D
D: store Anchor in config-backed state (effect To(Render, RenderSetPosition(a)))
D --RenderSetPosition(Anchor)--> V
V: anchor_rect(a, win, screen, pad) → move window. Pure mapping, 8 cases,
   unit-tested independent of any live X server.
```

---

## 3. MEETING EACH LOCKED SUCCESS CRITERION

1. **Background runtime, never a focus-stack surface.** Nothing we run is a Steam
   shortcut. `couch-renderd`'s only surface is a gamescope *external overlay*
   (atom-flagged), which by gamescope's design is not a focus-stack window.
   `couchd/rpcd/inputd` have no surface at all. Exiting a game returns to Big
   Picture untouched. **Structural, by process design.** ✔
2. **Local official RPC only.** *Only* `couch-rpcd` speaks Discord, *only* over
   `discord-ipc-0`, *only* the confirmed command set behind the `DiscordRpc`
   trait. No injection surface exists because rendering and input are in other
   processes that have no Discord access at all. ✔
3. **Chord opens the GUI, grabs focus, releases on dismiss.** `couch-inputd`
   detects the chord off the Steam-Input virtual keyboard, grabs the vkbd, emits
   `Chord`; `couchd` drives the menu; on dismiss/connect `couchd` sends
   `InputRelease` and `inputd` ungrabs → input returns to the game. ✔
4. **Steam-client-styled GUI over the active window.** `couch-renderd` renders the
   `MenuModel` with a Steam-ish theme onto the external-overlay window above the
   game surface. Theme lives entirely in the render domain. ✔
5. **Server → voice-channel browser, voice-only.** `RpcGetGuilds` →
   `RpcGetVoiceChannels`, filtered to `type==2` (and `13` if enabled) **inside
   `DiscordRpc::voice_channels`** so the filter exists in exactly one place. UI
   flow is the `ServerList → ChannelList` states. ✔
6. **Leave channel.** `RpcSelectVoiceChannel{None}` → `channel_id=null`. One
   command, one state transition. ✔
7. **Voice-activity overlay, anchorable to 8 positions.** `OverlayModel` driven by
   SUBSCRIBE voice/speaking events; `Anchor` enum has exactly the 8 positions;
   `anchor_rect` maps each to a pixel rect. ✔

---

## 4. UPGRADE & ISOLATION

**The general mechanism:** because each domain is a separate process behind a
versioned wire contract, "upgrade domain X" = build the new `couch-Xd` binary,
`systemctl --user restart couch-Xd`. systemd keeps the bus socket
(socket-activated), so on restart the new binary dials in, sends `Hello` with its
proto version, `couchd` revalidates against `major`, and the world rebuilds from
canonical state. **No other process is rebuilt, restarted, or even aware** beyond
seeing a `ServiceDown`/`ServiceUp` blip.

**Blast radius of a Discord-RPC change** (new command, RPC library swap, API
quirk fix): contained to `couch-rpcd` + possibly a *minor* bump in `couch-proto`
if a new `Command`/`Event` variant is added (additive → `minor` bump, old daemons
still parse). The renderer and input daemon are byte-identical and not restarted.
If Discord itself disconnects, only `rpcd` loops; `couchd` surfaces
"Discord disconnected" in the menu and the overlay clears — game and input are
oblivious. **Radius: 1 process (+ additive proto).**

**Blast radius of a renderer swap** (e.g. switch X11 → a future Wayland-layer
overlay, or change GPU backend): contained to `couch-renderd` behind
`OverlaySurface`. The `RenderShow/RenderOverlay/RenderSetPosition` commands and
the `MenuModel/OverlayModel` snapshots are the contract; as long as the new
renderer consumes those, nothing upstream changes. You can even run old and new
renderer side-by-side on a temp socket to A/B before flipping the unit. **Radius:
1 process.**

**Blast radius of an input-method change** (Steam Input vkbd → raw evdev, or a
different chord scheme): contained to `couch-inputd` behind `InputSource` +
`KeymapFsm`. It still emits `Event::Chord/Nav`. `couchd`'s UI FSM is written
against those abstract events, not against keycodes, so it is untouched. The
Steam Input *template* (a shipped asset) can change without touching any binary.
**Radius: 1 process (+ maybe a shipped .vdf asset).**

**What forces a coordinated upgrade (honest):** a **`major`** proto bump (a
breaking change to `Envelope`/`Command`/`Event` shape). Then `couchd` and the
affected daemon must be upgraded together — `couchd` will reject mismatched
`major` at `Hello`. This is the *intended* tripwire: breaking the contract is the
one thing that should be loud and atomic. Everything else is independent.

---

## 5. THE 3 BIGGEST HONEST RISKS / WEAKNESSES OF THIS APPROACH FOR THIS TOOL

1. **Process/IPC overhead is large relative to the actual workload.** This is, at
   heart, a single-user couch utility with maybe a dozen menu interactions per
   session and a low-frequency speaking-activity stream. Four daemons + an
   orchestrator + CBOR-framed unix sockets + systemd units + a versioned protocol
   is a lot of moving infrastructure for that. Latency on "open menu" now crosses
   three process boundaries (input→couchd→rpcd and back→render) where an in-proc
   design would be a function call. It's well within human-perceptible budgets
   (sub-ms IPC), but the *engineering* cost — five binaries to build, ship,
   version, and debug — is real and is the price of the isolation the user
   explicitly prioritized.

2. **The orchestrator is a single point of failure and a latent god-object.**
   Everything routes through `couchd`, and it holds the UI state machine. If it
   crashes mid-session the menu freezes (though the game is safe and domains hold
   their resources until it returns). More insidiously, because it's the only
   place that knows the *whole* flow, there's constant pressure to leak domain
   knowledge into it ("just special-case this Discord thing in the router"). The
   `UiState`/`UiEffect` purity discipline fights this, but it requires ongoing
   reviewer vigilance — the architecture *enables* clean boundaries but doesn't
   *enforce* that couchd stays dumb.

3. **Cross-process debugging and lifecycle race conditions.** A failure that
   spans domains (chord fires but menu never appears) now requires correlating
   logs across four journald units and reasoning about handshake/reconnect timing
   — e.g. the renderer restarting exactly as `couchd` sends a `RenderShow`, or
   the input grab/ungrab racing a `couchd` restart and leaving the virtual
   keyboard grabbed (input dead until manual recovery). These ordering bugs are
   *created by* the multi-process split; a monolith simply can't have them. The
   socket-activation + idempotent-snapshot + reconnect design mitigates most, but
   the grab/ungrab path is genuinely dangerous: a crash while holding the vkbd
   grab needs a guaranteed ungrab-on-exit (systemd `ExecStopPost` + a watchdog)
   or the user is stuck. This is the one place the isolation philosophy adds a
   *new class* of risk that didn't exist before.

---

## Appendix A — concrete build order

1. `couch-proto` + `couch-ipc` (+ an in-memory loopback `Connection` for tests).
2. `couchd` with a fake-event injector → exercise the `UiState` FSM with zero
   real domains.
3. `couch-rpcd` against `discord-ipc-0` (live-validate `SELECT_VOICE_CHANNEL`).
4. `couch-inputd` against the Steam Input template (live-validate chord→daemon).
5. `couch-renderd` external-overlay window + `anchor_rect` for 8 positions.
6. `couchcordctl install` + systemd units + socket activation; soak-test
   independent `restart` of each domain mid-session.
