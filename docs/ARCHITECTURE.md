# couchcord — Canonical Architecture

> Status: **CANONICAL.** This is the architecture of record, synthesized by the
> chief architect from three panel proposals (modular-monolith, event-driven-core,
> multi-process-services) and their three critiques. It takes the best of each and
> discards what the critiques exposed. Where a panel proposal said one thing and a
> critique falsified it, the critique wins and the reasoning is recorded inline.

The user's weighted-highest priority governs every call below:

> **Highly modular software, clear service/module boundaries, so upgrades are easy
> and domains are isolated. This outranks raw simplicity.**

We honor that by making **domain boundaries physical (a cargo workspace where
siblings cannot name each other)** while refusing modularity *ceremony* that buys
no isolation (the critiques' recurring finding). Modularity is spent on the
boundaries that actually churn, not on infrastructure cosplay.

---

## 0. Resolved decisions (post-panel, user-confirmed)

The three §8 open questions are settled; the body above/below stands, with these
bindings:

1. **Icons / network:** the constraint is now **"official Discord API only"**,
   not "local RPC only" — any network request is fine if it hits an official
   Discord surface. `cc-assets` fetches icons/avatars from `cdn.discordapp.com`
   (cached, with the initials-tile fallback for offline/unresolved). The
   no-network variant is dropped.
2. **Stage voice (`type == 13`): IN for v1.** `VoiceKind::Stage` is a first-class
   citizen; `cc-menu` carries the speaker/audience selection branch; the accepted
   type set defaults to `{2, 13}` in `cc-config`.
3. **Install privilege: `input`-group membership** (no `/etc/udev` root rule).
   `couchcordd install`/`doctor` checks group membership and instructs the user
   to `sudo usermod -aG input $USER` (one-time, then re-login) rather than
   dropping a root udev rule. The `assets/udev/` rule in §5 is therefore dropped.

---

## 1. Chosen process / deployment model — and why

### 1.1 The decision

> **One binary, `couchcordd`, run as a single `systemd --user` service.**
> Internally: a **cargo workspace of domain crates** that never name each other,
> wired in one composition root. Domains communicate over **explicit typed
> channels per producer→consumer edge** (NOT one broadcast bus), driven by **one
> async supervisor** on Tokio, with **blocking/FFI IO (X11, evdev) on dedicated
> threads** bridged to the async core by channels.

This is a **modular monolith with a typed-edge core** — the synthesis of
proposal #1's physical-crate boundaries and proposal #2's domain-typed message
discipline, with proposal #2's single broadcast bus replaced by per-edge channels
(its own critique killed the broadcast bus), and proposal #3's process split
rejected wholesale except as a *documented future swap* for exactly one crate.

### 1.2 Why this, and the named tradeoff vs. the two rejected models

**Rejected model A — Multi-process services (proposal #3).** Four daemons + an
orchestrator + CBOR + a versioned wire protocol + socket activation.

- *The tradeoff we are refusing:* hard, address-space fault isolation between
  domains, plus `systemctl restart <one-daemon>` as the upgrade verb.
- *Why we refuse it (from critique #3):*
  1. **It converts the one safety-critical invariant into a distributed one.**
     The controller grab/ungrab is the single resource that can *soft-brick the
     user's controller mid-game*. In a monolith, grab and ungrab are one lock with
     RAII `Drop`. Split across `couch-inputd` and `couchd`, the ungrab decision now
     depends on a second process being alive and a message being delivered. A
     dropped frame or a crashed orchestrator leaves the virtual keyboard grabbed
     and the game unplayable. This is the *opposite* of the spec's reason for
     existing ("never trap the user"). **Decisive.**
  2. **The "upgrade one process" headline is false for the upgrades that actually
     happen.** Adding `SET_VOICE_SETTINGS` (mute/volume from the overlay — a
     feature the spec itself lists) ripples across `couch-proto` → all 5 binaries
     rebuild → plus rpcd, plus couchd's FSM, plus renderd. The decomposition
     isolates *external resources* but not *features*, and features are what get
     upgraded. The shared `couch-proto` enum is a god-contract every binary depends
     on, so it re-couples the domains the user wanted isolated.
  3. **`couchd` is not domain-agnostic.** Its `UiState` holds `Vec<Guild>`,
     `ChannelId`, etc. — it *is* a Discord voice-browser. The proudest boundary is
     already breached.
  4. The restart-in-isolation verb runs almost exclusively *during development*,
     not during couch use, so the permanent runtime/operational tax buys a property
     this single-user tool barely exercises.

**Rejected model B — Single broadcast bus as the unit of modularity
(proposal #2, as literally specified).** One `tokio::broadcast<Msg>`; every module
receives every message.

- *The tradeoff we are refusing:* maximal pub/sub decoupling — any module can
  appear/disappear without a producer knowing.
- *Why we refuse it (from critique #2, confirmed by the proposal's own §5.1):*
  1. **The central isolation claim ("the enum is the only coupling; the bus is
     pure plumbing") is true at compile time and false at runtime.** A slow
     `cc-render` can `Lagged`-drop a `JoinedVoice`, silently desyncing the overlay.
     Discord's event rate now affects render correctness through a channel neither
     domain names — a leaky boundary by definition.
  2. **The fix re-couples it.** Per-message overflow policy ("coalesce
     `RenderIntent`, never coalesce `DiscordCommand`") puts *domain knowledge into
     the bus*, so the bus is no longer "pure plumbing" and no longer swappable.
  3. **Shipping the grab/release control loop over a lossy fire-and-forget channel
     is the single most dangerous under-specification** — a dropped
     `ReleaseNavigation` leaves the controller remapped with no failsafe.

We keep proposal #2's *genuinely good* ideas — **domain-typed sub-enums** instead
of a fat blob, the **pure, syscall-free menu state machine** as the most-tested
crate, and **rejecting internal multiprocess** — and discard only the single
broadcast bus, replacing it with explicit typed channels that name back-pressure.

**Why the monolith wins for *this* user's priority.** The deployment is one
artifact, so an upgrade is `cargo build && systemctl --user restart` — no
two-component version matrix, no wire-schema negotiation, no distributed grab
invariant. The modularity is **internal and compile-enforced** (siblings cannot
name siblings — CI-gated), which is *stronger* than a wire contract because the
compiler is the check. We get distributed-systems-grade *domain* isolation with
zero distributed-systems operational tax. That is exactly the trade the user
ranked highest (isolation/upgradability > raw simplicity), without paying for
isolation the tool never uses.

**The honest cost we accept (from critique #1 §3 and critique #2 §5.3):** a
monolith gives weaker *fault* isolation than its tidiness implies — a libX11
segfault or a uinput FFI abort takes the whole process down, and in-process
per-task "supervision" cannot catch an `abort`. We do **not** pretend otherwise.
Our answer is deliberately boring: **let systemd restart the whole binary**
(it does so in ~2s), and **isolate the one genuinely-FFI-risky domain — the X11
renderer — behind a trait whose pre-designed alternative impl is an out-of-process
renderer** we can promote later if X11 crashes prove frequent (§6.2, §7). We do
not build an in-process supervisor that the critiques proved cannot save us from
the only crash it exists to survive.

---

## 2. Domain map

Eight crates. Each has exactly one responsibility and one externally-observable
boundary. "Owns" means *no other crate may touch this resource or knowledge.*

| Crate | Single responsibility | Boundary (what crosses it) | External edge it owns |
|---|---|---|---|
| **`cc-core`** | The vocabulary: domain value types + the per-domain message enums + the boundary traits. **No logic, no IO.** | Types only. Everyone depends on it; it depends on no sibling. | none |
| **`cc-config`** | Load + validate config (`client_id`, theme, default anchor, keymap, voice-type set). | Returns a `Config` snapshot + a hot-swap handle. **Not on any message channel.** | the config file |
| **`cc-discord`** | Speak official local RPC. Owns the socket, framing, OAuth handshake, reconnect, the confirmed command set, **and the voice-channel filter**. Emits **domain events**, never RPC verbs. | `RpcClient` trait + a `VoiceEvent` stream. Hands out *only* voice channels and domain types — never JSON, never a socket, never a text channel. | `$XDG_RUNTIME_DIR/discord-ipc-0` |
| **`cc-assets`** | Resolve Discord icon/avatar *hashes* → cached image bytes via the Discord **CDN** (HTTPS). The one home for the asset pipeline the monolith proposal left undefined. | `AssetStore` trait: `hash → Option<Handle>`. Async, cached, best-effort. | Discord CDN over HTTPS |
| **`cc-input`** | Read the **Steam-Input virtual keyboard** (uinput/evdev). Discover the synthetic device, distinguish it from a real keyboard, debounce the chord, decode nav keys, and **grab/ungrab the *virtual* device** (never the physical pad, never `EVIOCGRAB` on the pad). Owns the grab *lifetime* via RAII. | `InputSource` trait emitting semantic `InputIntent`s. **Never names a Discord type or a pixel.** | `/dev/input/eventN` (Steam virtual kbd), `/dev/uinput` |
| **`cc-render`** | Own the gamescope **external-overlay** X11 window: create override-redirect, set `GAMESCOPE_EXTERNAL_OVERLAY`, discover the gamescope nested-X display, paint a declarative `Scene`, and compute 8-anchor geometry. Runs on its **own thread** (X11 is blocking/non-`Send`). | `OverlayRenderer` trait consuming an immutable `Scene` snapshot. **Never names a Discord type or an input key.** | gamescope nested X server |
| **`cc-menu`** | The **pure** state machine: `(State, Inbound) → (State, Vec<Outbound>, Scene)`. Owns "what the app does": which screen, selection, the 8-position cursor, the live roster model. **Zero IO, zero syscalls, no sibling-impl deps.** | Consumes inbound domain events; produces commands + a `Scene`. The only crate that is *fully* unit-testable with plain enums. | none (the only domain with no external edge) |
| **`couchcordd`** (bin) | Composition root + supervisor. The **only** crate that names concrete impls. Constructs each impl, wires the typed channels, owns the `tokio::select!` reactor, drives the IO threads, handles `sd_notify` + install/doctor. | Depends on every impl crate (the *only* one allowed to). | systemd `sd_notify` |

**Two boundary corrections forced by the critiques, applied here as law:**

1. **The voice-channel filter lives in `cc-discord`, not `cc-menu`.** Critique #1
   §1.1 proved the monolith's "filter is policy, put it in menu-state" decision is
   a *leak*: it splits "what is a voice channel" across two crates and pushes text
   channels the tool never uses across the boundary. `type == 2` (and optional
   `13`) is a **Discord fact**; `cc-menu` sees *only* voice channels. The set of
   accepted types is a `cc-config` value, so adding a future voice type is a config
   change, and the parse+filter stay co-located in one crate.

2. **The overlay roster is derived independently of menu state.** Critique #2 §A2
   showed routing the always-on HUD through `cc-menu` makes the "safest to iterate"
   crate also the one you cannot touch without risking the live overlay. `cc-menu`
   still owns the roster *view model*, but the roster's *source of truth* is the
   `VoiceEvent` stream, and the `Scene` carries `menu` and `overlay` as
   **independent layers** so the HUD renders with the menu closed and input
   released.

---

## 3. Key interfaces / traits at each boundary

All message/value types live in `cc-core`, are `#[non_exhaustive]`, and are split
into **domain-typed sub-enums** (critique #2's "good taste") so a Discord change
never appears in the input or render compile surface.

```rust
// ============================ cc-core: value types ============================
pub struct GuildId(pub u64);
pub struct ChannelId(pub u64);
pub struct UserId(pub u64);

pub struct Guild       { pub id: GuildId, pub name: String, pub icon: Option<AssetHash> }
pub struct VoiceChannel{ pub id: ChannelId, pub name: String, pub kind: VoiceKind }
#[non_exhaustive] pub enum VoiceKind { Guild, Stage }          // type==2 / type==13 ONLY
pub struct VoiceMember { pub user: UserId, pub name: String, pub avatar: Option<AssetHash>,
                         pub speaking: bool, pub muted: bool, pub deafened: bool }

#[non_exhaustive]
pub enum Anchor { TopLeft, TopCenter, TopRight,           // criterion 7: exactly 8
                  MidLeft,            MidRight,
                  BottomLeft, BottomCenter, BottomRight }

pub struct AssetHash(pub String);                          // CDN hash, resolved by cc-assets
```

```rust
// ===================== cc-core: domain-typed messages =========================
// Producer→consumer edges are explicit (see §4). No single fat bus.

#[non_exhaustive]                       // INPUT domain  (cc-input → cc-menu)
pub enum InputIntent { Chord, Up, Down, Left, Right, Confirm, Back, Dismiss, AnchorCycle }

#[non_exhaustive]                       // INPUT control (cc-menu → cc-input)
pub enum InputControl { Grab, Release } // logical capture of the virtual kbd nav keys

#[non_exhaustive]                       // DISCORD command (cc-menu → cc-discord)
pub enum DiscordCommand {
    Connect,
    ListGuilds,
    ListVoiceChannels { guild: GuildId },
    JoinVoice  { channel: ChannelId },
    LeaveVoice,                                   // → SELECT_VOICE_CHANNEL{null}
    SubscribeVoice   { channel: ChannelId },      // per-CHANNEL, not per-guild (critique #1 §1.4)
    UnsubscribeVoice { channel: ChannelId },
}

#[non_exhaustive]                       // DISCORD event (cc-discord → cc-menu)
pub enum DiscordEvent {
    Connected { user: UserId }, Disconnected { reason: DisconnectReason },
    Guilds(Vec<Guild>),
    VoiceChannels { guild: GuildId, channels: Vec<VoiceChannel> },   // already filtered
    JoinedVoice { channel: ChannelId }, LeftVoice,
    VoiceMembers { channel: ChannelId, members: Vec<VoiceMember> },
    SpeakingChanged { channel: ChannelId, user: UserId, speaking: bool },
}

#[non_exhaustive]                       // why Discord went away — drives recovery UX
pub enum DisconnectReason { ClientNotRunning, SocketClosed, AuthFailed, Timeout }
```

```rust
// ===================== cc-core: the four boundary traits ======================
// Each is the COMPLETE, SUFFICIENT contract for its domain. Note what is absent:
// no JSON, no xcb::Window, no evdev::Device, no socket.

#[async_trait]                          // cc-discord boundary
pub trait RpcClient: Send + Sync + 'static {
    async fn connect(&self, app: ClientId) -> Result<UserId, RpcError>;
    async fn guilds(&self) -> Result<Vec<Guild>, RpcError>;
    /// PRE-FILTERED to VoiceKind here, once, in the domain that knows the taxonomy.
    async fn voice_channels(&self, g: GuildId) -> Result<Vec<VoiceChannel>, RpcError>;
    async fn select_voice(&self, c: Option<ChannelId>) -> Result<(), RpcError>; // None = leave
    async fn selected_voice(&self) -> Result<Option<ChannelId>, RpcError>;
    /// Long-lived, per-CHANNEL. Emits onto the discord→menu edge (NOT returned as a
    /// side-channel — critique #1 §1.3: one seam, not two).
    fn subscribe_voice(&self, c: ChannelId) -> BoxStream<'static, VoiceEvent>;
}

pub trait InputSource: Send + 'static {  // cc-input boundary
    fn intents(&mut self) -> BoxStream<'static, InputIntent>;
    /// RAII-scoped capture of the VIRTUAL kbd's nav keys. Dropping the guard
    /// ALWAYS ungrabs — the soft-brick failsafe (critique #3 D.1) lives in the type.
    fn grab(&mut self) -> Result<NavGuard, InputError>;   // NavGuard: Drop → ungrab
}

#[async_trait]                          // cc-render boundary — ASYNC (critique #1 §4)
pub trait OverlayRenderer: Send + 'static {
    /// Discover the gamescope nested-X display + atom owner, retrying until present.
    async fn realize(&mut self) -> Result<(), RenderError>;
    /// Paint an immutable, idempotent Scene snapshot. Dropped frames self-heal.
    async fn draw(&mut self, scene: &Scene) -> Result<(), RenderError>;
    fn set_anchor(&mut self, a: Anchor);  // pure geometry; 8 cases; unit-tested
}

pub trait AssetStore: Send + Sync + 'static {  // cc-assets boundary
    /// Best-effort, cached. None → renderer draws an initials/placeholder tile.
    async fn resolve(&self, hash: &AssetHash, kind: AssetKind) -> Option<ImageHandle>;
}

pub trait ConfigSource: Send + Sync + 'static {  // cc-config boundary — NOT a message
    fn current(&self) -> Arc<Config>;             // ArcSwap read; not on any channel
    fn store_anchor(&self, a: Anchor);            // persist the 8-position choice
}
```

```rust
// ===================== cc-menu: the pure brain ================================
pub enum MenuState {
    Closed,                                           // overlay-only if connected
    Guilds      { guilds: Vec<Guild>, cursor: usize },
    Channels    { guild: GuildId, channels: Vec<VoiceChannel>, cursor: usize },
    Connected   { channel: ChannelId },               // menu may be closed; HUD lives
    Reposition  { from: MenuState_box },
}

pub struct Roster { pub channel: ChannelId, pub members: Vec<VoiceMember> }

/// The whole app logic is this one pure function. No await, no IO.
impl MenuEngine {
    pub fn on_input  (&mut self, i: InputIntent)  -> Step;   // Step{ cmds, controls, scene }
    pub fn on_discord(&mut self, e: DiscordEvent) -> Step;
    pub fn on_config (&mut self, c: Arc<Config>)  -> Step;
}

/// Declarative scene: menu and overlay are INDEPENDENT layers (critique #2 §A2).
pub struct Scene {
    pub menu:    Option<MenuView>,   // None when dismissed
    pub overlay: Option<Overlay>,    // Some whenever connected, regardless of menu
}
pub struct Overlay { pub anchor: Anchor, pub roster: Roster }
```

The asset pipeline (`AssetHash` in value types + `AssetStore` trait + `cc-assets`
crate) is the explicit home critique #1 §1.2 found missing. `cc-render` asks
`cc-assets` for an `ImageHandle` by hash; if `None`, it draws a placeholder, so
icons are a *graceful enhancement*, never a blocker — and `cc-render` still names
zero Discord types (it gets an opaque `ImageHandle`, not a CDN URL).

---

## 4. Communication / data flow

**Topology.** Explicit typed channels per producer→consumer edge — no broadcast,
no fan-out to uninterested modules:

```
                         ┌──────────────────── couchcordd (reactor) ────────────────────┐
                         │                                                               │
  [cc-input thread] ─intents()──►┐                                  ┌─► select_voice()   │
   Steam virtual kbd             │   InputIntent          DiscordCommand                 │
   /dev/input/eventN   ◄─grab()──┘        │                    ▲      │                  │
                         │  InputControl   ▼                    │      ▼                  │
                         │      ▲       ┌──────────┐  DiscordEvent  ┌──────────┐          │
                         │      └───────│ cc-menu  │◄───────────────│cc-discord│◄─socket─►│ discord-ipc-0
                         │   (pure FSM) │  engine  │── DiscordCmd ─►│  (async) │          │
                         │              └────┬─────┘                └────┬─────┘          │
                         │            Scene  │                  VoiceEvent│ (per channel) │
                         │                   ▼                           │               │
                         │              ┌──────────────┐  resolve(hash)  ▼               │
                         │   draw() ───►│  cc-render    │◄──────────── cc-assets ◄─HTTPS─►│ Discord CDN
                         │   (own X     │  thread       │              (cached)           │
   gamescope nested X ◄──┼── thread)    └──────────────┘                                 │
   GAMESCOPE_EXTERNAL_OVERLAY                                                             │
                         └───────────────────────────────────────────────────────────────┘
   cc-config: read via ConfigSource::current() (ArcSwap), NOT a channel (critique #2 §3 / #2 §B1)
```

Each edge is a bounded `tokio::mpsc` (or a thread↔async bridge channel) with a
**named overflow policy** stated at the channel, not hidden in a bus:
`DiscordCommand` = never drop (back-pressure the producer); `Scene` to renderer =
coalesce-to-latest (only the newest frame matters); `VoiceEvent` = coalesce
per-`(channel,user)` speaking state. The policy is local and visible.

### 4.1 Invoke menu (criterion 3)

```
Steam Input action-layer (chord held) → emits signal key on the VIRTUAL kbd
[cc-input] decodes → InputIntent::Chord ──► [cc-menu]
[cc-menu] Closed → Guilds(loading); returns Step{
    controls:[InputControl::Grab],          // cc-input takes a NavGuard (RAII)
    cmds:[DiscordCommand::ListGuilds],
    scene: Scene{ menu:Some(loading), overlay:<unchanged> } }
reactor dispatches: cc-input.grab() → NavGuard held;
                    cc-render.draw(scene);
                    cc-discord.guilds()  (async)
```

> **Where the input gate actually lives (critique #1 §4, critique #2 §C2,
> critique #3 D.3).** The thing that stops nav keys from *also* reaching the game
> is the **Steam Input action layer in the shipped controller template** — only
> that layer emits the nav keys, and only while it is active. The daemon's
> `Grab`/`NavGuard` is the *daemon-side* reading of those keys; it is **not** and
> **cannot be** the mechanism that prevents input-bleed into the game. We document
> this as law so no implementer reaches for window focus (which would violate
> criterion 1). The `NavGuard`'s `Drop` is the failsafe: if the daemon dies, the
> ungrab runs; the template's layer-pop is the controller-side counterpart, driven
> by the chord-release binding in the `.vdf`.

### 4.2 Browse servers → voice channels (criterion 5)

```
# browse servers
[cc-discord].guilds() resolves → DiscordEvent::Guilds(v) ──► [cc-menu]
[cc-menu] fills Guilds list → Scene(menu=guilds) → cc-render.draw
InputIntent::Up/Down ──► [cc-menu] moves cursor → Scene → draw   (pure, instant, no RPC)

# select a server → list its VOICE channels
InputIntent::Confirm on a guild ──► [cc-menu] Guilds → Channels(loading);
    Step{ cmds:[ListVoiceChannels{guild}], scene:menu=loading }
[cc-discord] GET_CHANNELS → FILTERS type==2(/13) HERE → DiscordEvent::VoiceChannels{..}
[cc-menu] fills voice-only list → Scene(menu=channels) → draw
```

The filter is in `cc-discord` (§2 correction 1): `cc-menu` never sees a text
channel, and "what is a voice channel" lives in exactly one crate.

### 4.3 Select / leave channel (criteria 5–6, `SELECT_VOICE_CHANNEL`)

```
# SELECT
InputIntent::Confirm on a channel ──► [cc-menu] Channels → Connected{channel};
    Step{ cmds:[ JoinVoice{channel}, SubscribeVoice{channel} ],
          controls:[ InputControl::Release ],         // hand control back to the game
          scene: Scene{ menu:None, overlay:Some{anchor, roster:loading} } }
reactor: cc-discord.select_voice(Some(channel));  cc-discord.subscribe_voice(channel);
         cc-input drops NavGuard → virtual-kbd nav released;  cc-render.draw(overlay-only)
[cc-discord] SELECT_VOICE_CHANNEL + SUBSCRIBE(voice_state, speaking) for THIS channel
→ DiscordEvent::JoinedVoice + VoiceMembers ──► [cc-menu] fills roster → draw overlay

# LEAVE
InputIntent::Confirm on "Leave" (or Back in Connected) ──► [cc-menu]
    Step{ cmds:[ LeaveVoice, UnsubscribeVoice{channel} ], scene: overlay:None }
[cc-discord] SELECT_VOICE_CHANNEL{channel_id:null} → DiscordEvent::LeftVoice
[cc-menu] Connected → Channels; roster cleared → draw
```

### 4.4 Render voice activity (criterion 7, live HUD)

```
While Connected, [cc-discord] receives SPEAKING_START/STOP + VOICE_STATE updates
  for the subscribed channel → DiscordEvent::SpeakingChanged / VoiceMembers ──► [cc-menu]
[cc-menu] folds into Roster (independent of menu open/closed) →
  Scene{ menu:<whatever>, overlay:Some{anchor, roster} } → cc-render.draw
# This path runs with the MENU CLOSED and INPUT RELEASED. overlay is its own layer.
# cc-render asks cc-assets.resolve(avatar_hash) lazily; None → initials tile.
# VoiceEvent edge coalesces redundant speaking flips; renderer coalesces to latest Scene.
```

### 4.5 Reposition overlay — 8 positions (criterion 7)

```
InputIntent::AnchorCycle ──► [cc-menu] advances Anchor through the 8-variant enum;
    Step{ scene: Scene{ overlay:Some{ new anchor, roster } } }
reactor: cc-render.set_anchor(anchor) → anchor_rect(a, win, screen, pad) recomputes x,y
         cc-config.store_anchor(anchor)        // persisted via ConfigSource, NOT a Msg
[cc-render].draw → window moves. Geometry math (8 cases) lives ONLY in cc-render,
unit-testable with no live X server. cc-menu only NAMES the position.
```

---

## 5. Crate / workspace layout

```
couchcord/
├─ Cargo.toml                      # [workspace]; resolver = "2"
├─ deny.toml                       # cargo-deny: enforces the sibling-dependency rule (§6.4)
├─ crates/
│  ├─ cc-core/                     # value types + domain-typed msg enums + boundary traits.
│  │                               #   Depends on NOTHING in the workspace. No logic.
│  ├─ cc-config/                   # Config load/validate + ArcSwap + store_anchor.
│  ├─ cc-discord/                  # discord-ipc-0: framing, OAuth, reconnect, command set,
│  │                               #   per-channel SUBSCRIBE, AND the voice-type filter.
│  ├─ cc-assets/                   # Discord CDN hash→bytes, cached. The asset pipeline.
│  ├─ cc-input/                    # Steam virtual-kbd discovery + decode + RAII NavGuard.
│  ├─ cc-render/                   # gamescope external-overlay X11 window (own thread) +
│  │                               #   display discovery + 8-anchor geometry + theme.
│  ├─ cc-menu/                     # PURE state machine. No IO. The richest test suite.
│  └─ couchcordd/   (bin)          # composition root + reactor + IO-thread drivers +
│                                  #   sd_notify + `install` / `doctor` subcommands.
├─ assets/
│  ├─ systemd/couchcordd.service   # ~/.config/systemd/user/; WantedBy=graphical-session.target
│  ├─ steam-input/couchcord.vdf    # the chord + nav action-layer template (the input gate)
│  └─ udev/99-couchcord-uinput.rules
└─ docs/
   ├─ ARCHITECTURE.md              # this file
   └─ panel/                       # the 3 proposals + 3 critiques this synthesizes
```

Dependency spine (CI-enforced): everyone → `cc-core`; `cc-core` → no one; siblings
→ **never** each other; only `couchcordd` may name an impl crate.

```
                         cc-core  ◄──────── all crates depend on it; it depends on none
                            ▲
   ┌────────┬───────────┬───┴────┬──────────┬──────────┐
 cc-config cc-discord cc-assets cc-input cc-render  cc-menu     (siblings: zero edges between them)
   ▲        ▲           ▲          ▲         ▲          ▲
   └────────┴───────────┴──────────┴─────────┴──────────┘
                    couchcordd (bin)  ◄── the ONLY crate that names impls
```

---

## 6. Upgrade & isolation analysis (the headline feature)

Three layered guarantees: **(a)** compile-time isolation — siblings cannot name
each other, so there is no edge to break; **(b)** type-level stability — all
`cc-core` types are `#[non_exhaustive]` and **domain-split**, so a Discord change
does not even appear in the input/render compile surface; **(c)** one injection
point — impls appear only in `couchcordd`, so *replacing* one is a single
`Box::new` line. Blast radius below is stated as **files that must change**.

### 6.1 A **Discord-RPC change**

*Scenario: Discord adds a field, you move from raw IPC to a maintained RPC lib, the
framing changes, or auth/reconnect/token-refresh behavior evolves.*

- **Internal change (no new capability):** `cc-discord/` only. The `RpcClient`
  signature is unchanged → **blast radius = 1 crate, 0 other files.** `cc-menu`,
  `cc-render`, `cc-input` cannot break: they never saw the socket, and they speak
  *domain* verbs (`JoinVoice`), never `SELECT_VOICE_CHANNEL`.
- **New capability** (e.g. `SET_VOICE_SETTINGS` mute/volume — the spec lists it):
  add one `#[non_exhaustive]` variant to `DiscordCommand`/`DiscordEvent` in
  `cc-core` (additive, nothing breaks) + implement it in `cc-discord` + consume it
  in `cc-menu`. **Blast radius = 3 files, all additive.** Critically, *unlike the
  multi-process model*, `cc-render` and `cc-input` do **not** rebuild from a
  Discord-only variant, because the enums are domain-split (this is the precise
  defect critique #3 §A2 found in proposal #3's shared `couch-proto`).
- **The realistic Discord churn is auth/reconnect**, not verb-renaming
  (critique #2 §C3). That logic is wholly inside `cc-discord`; `DisconnectReason`
  is the contract that lets `cc-menu` show a recovery banner. Blast radius for a
  reconnect-policy change = **1 crate.**

### 6.2 A **renderer change**

*Scenario: X11/Cairo → `wgpu`; or gamescope external-overlay → Wayland layer-shell;
or — the big one — move the renderer **out of process** to contain X11 FFI crashes.*

- In-process backend swap or windowing-system swap: edit/replace `cc-render/`,
  flip **one line** in `couchcordd`. The `Scene` contract is unchanged → `cc-menu`
  untouched. **Blast radius = 1 crate + 1 line.**
- **Out-of-process renderer (the fault-isolation escape hatch):** because
  `cc-render` already consumes an **immutable, idempotent `Scene` snapshot** over a
  channel, promoting it to a separate `couch-overlayd` process is "serialize the
  same `Scene` over a unix socket." Only `couchcordd`'s wiring + a new thin
  `cc-render-ipc` shim change; `cc-menu` and every other domain are untouched
  because they only ever produced a `Scene`. **Blast radius = 1 new shim + wiring.**
  This is our deliberate answer to the monolith's one honest weakness (FFI fault
  containment, critique #1 §3 / critique #2 §5.3): we *pre-design* the split for
  the single riskiest edge and pull it only if X11 crashes prove frequent — we do
  **not** pay for it up front, and we do **not** build an in-process supervisor the
  critiques proved cannot catch the crash anyway.

### 6.3 An **input-method change**

*Scenario: Steam Input changes its virtual-kbd behavior; you add a raw-evdev
fallback for non-gamescope sessions; or a network/phone trigger.*

- New `InputSource` impl in `cc-input/` (or a sibling `cc-input-evdev`). The trait
  yields **semantic `InputIntent`s**, not keycodes, so `cc-menu`'s nav logic is
  immune to how the intent was produced. **Blast radius = 1 crate + 1 line.**
- The `EVIOCGRAB`-avoidance invariant and the RAII `NavGuard` failsafe are
  localized to this crate and documented; a new source either honors them or is a
  knowingly different strategy — either way no other domain is touched. The
  **Steam Input template** (`assets/steam-input/couchcord.vdf`) can change with
  **zero** binary changes, because the input *gate* lives there (§4.1), not in code.

### 6.4 A **config change**

*Scenario: add a theme color, a new keybind, a second accepted voice type.*

- Edit `cc-config/`'s schema + whoever reads that field. Config is read via
  `ConfigSource::current()` (an `ArcSwap` snapshot) — it is **deliberately not a
  message on any channel** (critique #2 §B1 / critique #3: routing config through a
  contract churns the one crate that must stay small). So a new theme color touches
  `cc-config` + `cc-render`; a new accepted voice type touches `cc-config` +
  `cc-discord`. **Blast radius = 1–2 crates, never the whole graph.** Adding a
  config field never recompiles the message contract and never ripples to unrelated
  domains.

> **Enforcement, not discipline.** The sibling-dependency rule is mechanical: a
> `cargo-deny`/`xtask` check fails CI if any `cc-*` domain crate's `Cargo.toml`
> names another `cc-*` domain crate. That single gate is what makes every blast
> radius above *provable* rather than aspirational.

---

## 7. Phased build order

Tied directly to the two SPEC live-validation items. The governing rule (from all
three critiques): **spike the two unproven domains FIRST and do not freeze their
traits until the spikes pass on real hardware.** Lock the three understood traits
early; design the two risky traits *after* reality is observed.

**Phase 0 — De-risk gate (cheapest possible kills first).**
- `couchcordd doctor` skeleton that checks: is `discord-ipc-0` present? is the
  Steam virtual kbd discoverable and distinguishable from a real keyboard? does the
  gamescope nested-X display exist and does it accept `GAMESCOPE_EXTERNAL_OVERLAY`?
  (critique #1 §5 promotes this from a footnote to a hard gate — it is the cheapest
  de-risk of criterion 1.)

**Phase 1 — Frozen, understood core.**
- `cc-core` value types + domain enums; `cc-config`; `cc-menu` (pure, TDD, full
  flow coverage with a `MockBus`/recorded-`Step` harness). These three traits are
  *understood* — lock them now.

**Phase 2 — LIVE VALIDATION #1: `SELECT_VOICE_CHANNEL` end-to-end.**
- Thin, ugly `cc-discord` against `discord-ipc-0` with the **registered personal
  app** `client_id`: `AUTHORIZE`→token→`AUTHENTICATE`, `GET_GUILDS`,
  `GET_CHANNELS`+filter, then **`SELECT_VOICE_CHANNEL` (join), `{null}` (leave),
  per-channel `SUBSCRIBE`**. Drive it from a stub harness printing events.
  **Freeze `RpcClient` only after this passes** (and fix the per-channel
  subscription granularity here — critique #1 §1.4). Also prove the
  Discord-client-died recovery path (`DisconnectReason::ClientNotRunning`) since
  that is the failure most likely to push the user back to the bad pattern
  (critique #3 D.4).

**Phase 3 — LIVE VALIDATION #2: Steam-Input keyboard flow mid-game.**
- Import `assets/steam-input/couchcord.vdf`; thin `cc-input` reads the virtual kbd
  **while a real game runs in gamescope**. Validate: correct device discovery,
  the chord using **unmasked** keys (gamescope masks left-Windows — SPEC §62),
  nav keys arriving, and — the safety property — that dropping the `NavGuard`
  ungrabs. Confirm the **action layer is the input gate** and pops on chord-release
  in the template. **Freeze `InputSource` only after this passes.**

**Phase 4 — Renderer + assets (understood enough to build now).**
- `cc-render`: external-overlay window on its own thread, retrying display
  discovery, painting a static `Scene`, 8-anchor geometry (proves criteria 1+4+7).
  `cc-assets`: CDN hash→bytes with a placeholder fallback so icons never block.

**Phase 5 — Compose + ship.**
- Wire all crates in `couchcordd` over the typed-edge channels; `sd_notify`
  `Type=notify` health; `couchcordd install` (units, udev rule via a clearly
  privilege-aware step — see Open Question 3, critique #1 §5) + `doctor`.

**Phase 6 — (deferred insurance, only if needed).**
- Out-of-process `couch-overlayd` renderer split (§6.2), pulled only if real X11
  crash frequency justifies it.

---

## 8. Open questions for the user

1. **Guild/user icons — fidelity vs. the "official local RPC only" line.** Discord
   RPC returns icon/avatar *hashes*, not images; rendering them (criteria 4 & 7)
   means an HTTPS fetch to the Discord **CDN** (`cdn.discordapp.com`). That is the
   standard public CDN, not the local IPC socket, but it *is* network IO. We have
   designed `cc-assets` to own it with a **graceful placeholder fallback** (initials
   tiles when offline/unresolved). **Do you accept the CDN fetch, or do you want a
   strictly-no-network build that ships initials/color tiles only?** (This is the
   one place the architecture brushes the "local only" constraint and it is your
   call to make.)

2. **STAGE_VOICE (`type == 13`) — in or out of v1?** The filter, `VoiceKind`, and
   the join/leave flow are the same as `GUILD_VOICE`, but Stage channels have a
   speaker/audience distinction and a different join sub-flow. Including them is a
   `cc-config` flag flip + a `cc-menu` selection branch. **Ship v1 with Stage
   channels included, or `GUILD_VOICE` only and add Stage later?**

3. **Privilege boundary for install (udev rule).** Reading `/dev/uinput` and the
   Steam virtual keyboard cleanly wants a udev rule in `/etc/udev/rules.d/`, which
   needs **root** — but `couchcordd` is a `--user` daemon (critique #1 §5 flagged
   this contradiction). **Which do you want:** `couchcordd install` prints the udev
   rule + the one `sudo` command for you to run yourself (explicit, no hidden
   escalation), or it attempts a `pkexec`/`sudo` step, or we rely solely on
   membership in the `input` group (simpler, slightly broader access)? This is a
   one-time setup decision but it sets the security posture.
