# Critique — Multi-Process Services Proposal

Reviewer stance: hard-nosed, skeptical. The proposal is well-written and unusually
honest (its own §5 pre-concedes three real problems). That honesty is not a free
pass — several of its self-assessments are too kind, and the design has leaks and
gaps that its own narrative glosses. Graded against the **user's four weighted
priorities** (modularity, clean boundaries, ease of upgrades, domain isolation)
and against the **spec's hard constraints**.

---

## A. Where the boundaries are actually leaky

The proposal's central claim (§2.1, §2.2) is that `couch-proto` is "the ONLY
coupling point" and that domains share "nothing else." That is not true as
designed.

1. **`couchd` is not domain-agnostic — it owns Discord-shaped state.** §2.3's
   `UiState` enum literally contains `Vec<Guild>`, `Vec<VoiceChannel>`,
   `ChannelId`, `GuildId`. The proposal claims (line 294) couchd "Holds *no* domain
   knowledge (doesn't know what a 'voice channel' is beyond passing it through)."
   That is contradicted by its own `UiState::ChannelList { guild, chans, sel }` and
   `Connected { channel }`. The orchestrator's state machine **is** a Discord
   voice-browser. A change to the Discord domain model (e.g. Stage channels need a
   different selection sub-flow, or threads, or a "favorites" concept) ripples
   directly into `couchd`'s FSM. The boundary the proposal is proudest of is the
   one it has already breached. This is the single biggest honesty gap in the
   document.

2. **`couch-proto` is a god-contract, so every domain change touches the one
   shared crate.** The proposal frames a `minor` proto bump as cheap, but look at
   the `Command`/`Event` enums (§1.4): adding *any* new Discord capability
   (e.g. mute/deafen via `SET_VOICE_SETTINGS`, which the spec lists as available)
   means editing `couch-proto`, which every one of the five binaries depends on, so
   **all five recompile and must be re-shipped together**. The "Discord change
   can't force a recompile into the renderer" claim (line 280) is false at the
   build level — they share `couch-proto`, and `Command`/`Event` are not split per
   domain. A genuinely isolated design would give each domain its own command/event
   sub-crate (`couch-proto-rpc`, `couch-proto-render`…) so the renderer's build
   graph never sees a Discord-only variant. As written, the "one enum" is a
   convenience that re-couples the domains the user wanted isolated.

3. **The `DiscordRpc`/`OverlaySurface`/`InputSource` traits are intra-process and
   carry no isolation weight.** They live *inside* each daemon (§2.3). They are
   good unit-test seams, but the proposal repeatedly cites them as if they
   contribute to *domain isolation* (the cross-process property). They do not — a
   process boundary already gives you that. Presenting in-process trait objects as
   part of the isolation story inflates the modularity claim.

4. **Position/anchor state is owned in two places.** §1.4 lists
   `RenderSetPosition(Anchor)` as a command to the renderer, §2.4 says couchd
   stores the Anchor in "config-backed state," and §2.3 puts `anchor_rect` in the
   renderer. So the *current anchor* is authoritative in couchd, the *pixel mapping*
   is in renderd, and the *persisted default* is in config (§1.3, read by renderd's
   own `[section]`). On a renderer restart, who replays the last anchor — couchd
   (which must remember to) or renderd (from config, possibly stale)? Under-specified,
   and a classic split-brain seam.

---

## B. Where an upgrade ripples across domains (the user's stated #1 goal)

The §4 "blast radius: 1 process" claims are the headline selling point. They hold
only for the narrow cases chosen. The realistic upgrades ripple:

- **Adding a feature that the spec itself already contemplates.** Spec lists
  `SET_VOICE_SETTINGS` / `SET_USER_VOICE_SETTINGS` (per-user mute/volume from the
  overlay is the obvious next feature). Wiring that requires: a new `Command`
  variant (couch-proto, → all 5 rebuild), a new `UiState` branch or overlay
  interaction (couchd), a new `DiscordRpc` method (rpcd), AND a new overlay
  affordance (renderd). That is a **4-domain ripple** for one feature, on the
  architecture explicitly sold as "upgrade one process." The decomposition isolates
  *external resources* well but does not isolate *features* — and features are what
  actually get upgraded.

- **Renderer "side-by-side A/B on a temp socket" (line 535) is hand-waved.** couchd
  is a *star server* with a static `ServiceId::Render` registry slot (§1.4). Running
  two renderers means two connections claiming the same `ServiceId`, which the
  registry isn't designed for (one slot per id). The A/B story needs a second
  ServiceId or a routing key the proposal never defines. As stated it won't work.

- **The `major`/`minor` negotiation is the right idea but over-applied.** For a
  single-user tool where all five binaries are built from one workspace at one
  version and installed together by `couchcordctl install`, the proto-version
  handshake guards against a scenario that effectively never occurs in production:
  mixed-version daemons. You ship a workspace; everything is the same version.
  The negotiation is real engineering effort (and a `Hello`/`Welcome` round-trip on
  every connect) defending a multi-tenant/rolling-deploy property this tool does
  not have. Keep a single `proto_version: u32` equality check; delete the
  major/minor lattice.

---

## C. What's over-engineered for a one-user couch tool

The spec is explicit: "single-user couch tool," and that modularity "outranks raw
simplicity" — but outranking simplicity is not the same as *ignoring* it, and
several choices buy modularity the user can't use:

1. **Five binaries + systemd target + socket activation + CBOR + versioned bus**
   for ~a dozen interactions per session (the proposal admits this in §5.1). The
   damning part is not the overhead — it's that the isolation it buys is **rarely
   exercised**: how often will a couch user `systemctl --user restart
   couch-renderd` mid-game? The design optimizes for independent restart, a verb
   that in practice runs during *development*, not use. A dev-time concern is being
   paid for with permanent runtime and operational complexity.

2. **Socket activation is justified by a "thundering herd" of reconnects (line
   104) that cannot happen with four clients.** Four daemons reconnecting to a
   restarted couchd is not a herd; a 500ms backoff loop handles it trivially. The
   socket-activation machinery is cargo-culted from server architecture.

3. **`Ping`/`Pong` liveness over unix sockets (§1.4) duplicates what systemd and
   the kernel already tell you.** A dead peer on a unix `SOCK_STREAM` yields EOF/
   EPIPE immediately; systemd already tracks process liveness. App-level heartbeats
   add a registry-timeout state machine to detect something the transport reports
   for free.

4. **`MemoryMax`, sandboxing namespaces, `RestrictAddressFamilies`, defense-in-
   depth capability scoping (§1.2)** are presented as "free isolation" that
   "doubles as documentation." For a single trusted user running their own tool on
   their own machine, this is security theater against a threat model that doesn't
   exist. It's not harmful, but calling it a design *win* mistakes ceremony for
   value.

**What is genuinely worth keeping:** the split of the three external-resource
domains (Discord socket / X11 surface / uinput device) into separate units **is**
defensible — not for runtime isolation, but because the failure modes and the
privilege surfaces genuinely differ, and because a renderer or RPC crash should not
take down input grab/ungrab (the one path that can soft-brick the controller). That
specific isolation earns its keep. The orchestrator-as-separate-process does not.

---

## D. Under-specified or won't-actually-work against the spec's hard constraints

This is where skepticism bites hardest. The proposal treats the two items the spec
flags for **LIVE validation** as solved background detail and builds heavy
abstraction on top of unproven mechanism.

1. **The grab/ungrab path can soft-brick the controller — and the design adds new
   ways to trigger it.** The proposal itself flags this (§5.3) but understates it.
   `couch-inputd` grabs the *virtual keyboard* on chord and ungrabs on
   `InputRelease` — but the *decision* to release lives in `couchd` (the
   `Connected` transition emits `To(Input, InputRelease)`, §2.4). So the ungrab is
   now **cross-process and depends on couchd being alive and the message being
   delivered**. If couchd crashes, or the bus frame drops, or inputd restarts while
   couchd thinks it's still connected, the vkbd grab can persist with no game input.
   In a monolith, grab and ungrab are the same lock in the same process with RAII /
   `Drop`. The multi-process split **converts a local invariant into a distributed
   one** for the single most dangerous resource in the system. `ExecStopPost` +
   watchdog (line 587) covers *crash*, not *logic*/*delivery* failure. The release
   decision must not cross a process boundary; this argues for input grab-lifetime
   to be owned next to whatever decides to release it.

2. **Steam Input as keyboard + gamescope masking is treated as a black box behind
   `InputSource`, but the spec's open question is exactly whether the keys arrive.**
   Spec line 62 warns gamescope masks left-Windows and "use unmasked keys." The
   proposal never names the chord/nav keycodes, never addresses how `inputd`
   *finds* the right uinput node (Steam creates virtual devices with unstable
   names/numbers — "/dev/input/eventN" in the diagram is a hand-wave), and never
   says how `inputd` distinguishes the Steam virtual keyboard from a real keyboard
   the user might also have. The `InputSource` trait is drawn as if the hard part
   (device discovery, hotplug, gamescope key masking, distinguishing the synthetic
   device) is solved; it's the spec's #2 live-validation risk and the abstraction
   hides rather than confronts it.

3. **`GAMESCOPE_EXTERNAL_OVERLAY` ownership and surface lifecycle vs. the input
   grab is not reconciled with criterion 1/3.** The proposal asserts (§3.1) the
   external overlay "is not a focus-stack window," correctly. But criterion 3
   requires the chord to **grab input and hold focus until dismissed**. With input
   mediated as a *keyboard grab* (not window focus) and rendering as a *non-focus
   external overlay*, there is **no focused window at all** while the menu is open —
   which is consistent with "never a focus-stack surface," but the proposal never
   shows that arrow-key nav actually reaches `inputd` *while gamescope's focus stays
   on the game*. If Steam Input's action layer is emitting keys, do they go to the
   game (which is focused) or to the overlay? The answer is "inputd grabs the vkbd
   so they don't leak to the game" — but grabbing an evdev device is global; it
   doesn't route to the overlay either. The overlay isn't reading input; `inputd`
   is. That actually works, but the document never closes this loop, and it's the
   crux of the whole "never a focus-stack window" + "chord grabs focus" tension. It
   needs to be stated explicitly, because a careless implementer will reach for
   window focus and violate criterion 1.

4. **"Discord runs background, out of scope" leaves the worst real-world failure
   unhandled.** §1.5 says if Discord isn't up, rpcd loops and reports
   `RpcDisconnected`. But the *common* couch failure is Discord-the-client crashing
   or the `discord-ipc-0` socket being stale/half-open after a Discord update. The
   proposal has `RpcDisconnected` as an event but no recovery contract: does the
   user get told to relaunch Discord? Can couchcord relaunch it? The spec's whole
   reason-for-being is avoiding focus traps from Discord-as-shortcut; the recovery
   UX when background Discord dies is precisely the scenario most likely to push a
   frustrated user back toward the bad old pattern. Under-specified for the highest-
   value reliability path.

5. **Overlay redraw on `VoiceStateUpdate` is "throttled in couchd" (§2.4 line
   468) — but coalescing speaking events is domain logic leaking into the
   orchestrator.** couchd is supposed to be dumb routing + UI FSM. Frame-rate
   coalescing of a Discord event stream is render/domain concern. Either it belongs
   in rpcd (debounce the source) or renderd (drop redundant frames — which the
   proposal already says are idempotent, so renderd can self-throttle). Putting it
   in couchd is another knowledge leak into the god-object §5.2 warns about.

---

## E. Internal contradictions / honesty audit

- §2.2 line 294 ("couchd holds no domain knowledge") directly contradicts §2.3's
  Discord-typed `UiState`. Pick one. (It can't be the former.)
- §2.1 line 280 ("a Discord-RPC change cannot force a recompile-coupling into the
  renderer") contradicts the single shared `couch-proto` enum that the renderer
  depends on. Additive Discord variants recompile renderd.
- §4 "blast radius: 1 process" is true for the *resource-swap* examples chosen and
  false for the *feature-add* examples that dominate real maintenance.
- §5 is commendably honest, but §5.1 frames the overhead as merely "the price of
  isolation the user prioritized." The sharper truth is that *some* of the overhead
  buys isolation the user wanted, and *some* (the orchestrator process, socket
  activation, proto version lattice, ping/pong, sandboxing) buys properties this
  single-user tool will never use. The document doesn't separate the two.

---

## Top 3 changes that would most improve it

1. **Collapse `couchd` into the renderer (or make it a library, not a process), and
   own the input-grab lifetime next to the release decision.** The orchestrator is a
   single-point-of-failure god-object holding Discord-shaped UI state, and its
   separateness is what turns the controller grab/ungrab invariant into a fragile
   distributed one (D.1). Keep three external-resource daemons (rpc/render/input)
   for the failure-isolation that genuinely matters; fold orchestration+UI-FSM into
   one of them as in-process logic so the menu state machine and the input-release
   decision can't be severed by an IPC failure. This directly serves the user's
   isolation goal while removing the design's most dangerous race.

2. **Split the contract per domain instead of one shared `couch-proto` enum.** Give
   each domain its own command/event types (and ideally its own crate), so adding a
   Discord capability never recompiles or re-ships the renderer/input daemon. This
   is what actually delivers the "upgrade one domain in isolation" property the
   document claims but doesn't have today, and it removes the `UiState`/`couch-proto`
   re-coupling. Drop the major/minor version lattice for a single `u32` equality
   check, since all binaries ship as one workspace version.

3. **Promote the two spec-flagged live-validation risks from black-boxed traits to
   first-class, specified mechanism *before* building the abstraction layers:**
   (a) Steam-Input vkbd device discovery, gamescope key masking, real-vs-virtual
   keyboard disambiguation, and concrete chord/nav keycodes; (b) the
   guaranteed-ungrab contract (RAII + ExecStopPost + couchd-independent failsafe)
   and the Discord-client-died recovery UX. Build the input spike (Appendix A step
   4) and an RPC `SELECT_VOICE_CHANNEL` spike *first*; if either mechanism doesn't
   hold, the four-daemon scaffolding is premature.
