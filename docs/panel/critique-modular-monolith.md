# Critique — "The Modular Monolith" proposal

A skeptical review against the user's stated priorities (modularity, clean
boundaries, **ease of upgrades**, **domain isolation**) and against the SPEC's
hard constraints (official local RPC only, gamescope external-overlay rendering,
Steam-Input keyboard mediation, Rust, never a focus-stack window, single-user
couch tool).

The proposal is well-written and self-aware — §5 pre-empts three real risks. But
a good prose defense is not the same as a sound boundary design, and several of
its central modularity claims do not survive contact with the SPEC's mechanics.
Below I attack the specifics.

---

## 1. The boundaries are leakier than the diagram claims

### 1.1 `menu-state` owns the voice-channel filter — that is a real boundary leak, sold as a feature

§2.6 and §4.2 proudly relocate the voice-channel filter (`type==2 / type==13`)
out of `discord-rpc` and into `menu-state`, calling it "policy, not mechanism."
This is the proposal's signature boundary decision and it is **backwards** for the
user's priorities.

- The SPEC defines voice-channel filtering as a *Discord RPC fact* (hard
  constraint: "channel `type == 2` (GUILD_VOICE), optionally `13`
  (STAGE_VOICE)"). The mapping from raw `type` integers to `ChannelKind` already
  lives in `discord-rpc` (it must — that crate parses the JSON). So the knowledge
  is now **split across two crates**: `discord-rpc` decides what counts as
  `GuildVoice`, and `menu-state` decides that `GuildVoice` is the thing to show.
  A change to "what is a voice channel" (Discord adds a new voice type) now
  touches `discord-rpc` (parse) *and* `menu-state` (filter). That is the exact
  cross-domain ripple §4 claims the architecture prevents — and the proposal
  introduced it on purpose.
- Worse, it forces `discord-rpc.channels()` to return **all** channels including
  text channels the tool will never use, pushing Discord-shaped noise across the
  boundary into the pure brain. The "pure state machine" now has to know about
  channel taxonomy. That is mechanism bleeding into policy, not the reverse.
- The honest fix is the opposite of what the proposal argues: filtering is
  mechanism, belongs in `discord-rpc`, and `menu-state` should only ever see
  voice channels. The proposal's own §4.2 example ("if Discord adds a voice type,
  one-line policy change") is the tell — that change should be a `discord-rpc`
  change, full stop, and the fact that it lands in `menu-state` is a leak.

### 1.2 `menu-state` is not actually IO-free — it owns the icon/avatar problem the proposal never mentions

Criterion 4 (Steam-styled GUI) and criterion 7 (who's in / who's speaking) imply
guild icons and user avatars. `IconRef` appears in `core` (§2.2) and then
**vanishes**. Who fetches the icon bytes? Discord RPC returns icon *hashes*, not
images; resolving them is an HTTPS fetch to Discord's CDN — which is *not* on the
`discord-ipc-0` socket and arguably brushes the "official local RPC only"
constraint (it's the standard CDN, but it is network IO the architecture has no
home for). There is no trait for it, no crate for it, no event for it. Either:

- the GUI ships without icons (then say so — it changes criterion 4's fidelity), or
- some crate grows an HTTP client, and the cleanest-looking candidate is
  `overlay-render` (it needs the pixels) — at which point `overlay-render`
  depends on a Discord CDN URL scheme, and the "renderer knows nothing about
  Discord" claim in §4.3 is false.

This is **under-specified** and it is not a corner case — it is half of two
success criteria. A boundary design that has no answer for the asset pipeline has
not finished drawing its boundaries.

### 1.3 The `subscribe_voice` trait method contradicts the bus

§2.3 `RpcClient::subscribe_voice(&self, guild) -> BoxStream<VoiceEvent>` hands a
stream **directly** back to the caller. But §1.3 insists "no domain ever holds a
concrete handle to another domain" and that domains communicate *only* through
the bus. A `BoxStream` returned to the binary, then re-emitted onto the broadcast
bus, is a second, parallel communication channel that bypasses the bus discipline
the whole §1.3 is built on. Either the bus is the one true seam (then
`subscribe_voice` should emit onto it, not return a stream) or it isn't. The two
sections describe two different architectures. This is exactly the kind of
ambiguity that, in a "clean boundaries" design, becomes a maintenance argument
later.

### 1.4 `SUBSCRIBE` needs a channel, and the proposal subscribes by guild

The SPEC's RPC surface subscribes to **voice state + speaking events**, which in
the official RPC are per-**channel** (`VOICE_STATE_*` / `SPEAKING_START/STOP`
events are scoped to the channel you're connected to or watching), not per-guild.
`subscribe_voice(guild)` and `Subscribe(GuildId)` (§2.2 Command) are modeling the
subscription at the wrong granularity. This is a correctness gap that will force a
trait-signature change during build — i.e., it breaks the very "design the trait
once" promise for the one domain (Discord) the user most wants stable.

---

## 2. Where upgrades actually ripple across domains

The §4 "blast radius" tables are the proposal's centerpiece and they are
optimistic. The clean cases (swap renderer behind unchanged `ViewModel`) are
genuinely clean. The cases that matter for *this* tool are not:

- **A `ViewModel` change ripples to two crates by construction.** The `ViewModel`
  type lives in `core`, is produced by `menu-state`, and consumed by
  `overlay-render`. Any new thing the UI must show — a connection-error banner, a
  speaking-ring animation state, an avatar slot — changes `core` + `menu-state` +
  `overlay-render`. That is three crates for any UI feature, which is most of the
  remaining roadmap. The architecture optimizes for the rare event (swap the
  whole renderer) and taxes the common event (add a UI element). For a one-user
  couch tool, the common event dominates. `#[non_exhaustive]` does not save you
  here: adding a field to `ViewModel` that the renderer must draw is inherently a
  coordinated change, non_exhaustive or not.
- **The two LIVE-validation unknowns sit on the two load-bearing traits.** §5
  Risk 3 admits this, but it understates the consequence for the *user's* top
  priority. The user ranked "easy upgrades / isolated domains" highest, and the
  proposal locks `RpcClient` and `InputSource` as the isolation seams — yet both
  are explicitly unproven (SPEC §"need LIVE validation"). §1.4 above already shows
  one signature is wrong. You cannot claim "stable upgrade seams" for two domains
  whose shape you are still guessing. The proposal's own Appendix A hedge
  ("design those two traits last") is correct and quietly concedes that the
  headline modularity claim does not yet apply to two of the four domains.

---

## 3. Over-engineered for a single-user couch tool

The SPEC says modularity outranks raw simplicity — but it also says **single-user
couch tool**, and several mechanisms are distributed-systems machinery with no
payoff at this scale:

- **The split `Event` (broadcast) / `Command` (mpsc) bus with a central reactor
  routing every command** is a meaningful amount of indirection for a program
  whose entire job is: read ~5 key intents, call ~6 RPC methods, repaint one
  window. The proposal even admits (§5 Risk 2) the single reactor becomes "a
  single point of latency coupling" against three blocking/synchronous IO models
  (x11rb, evdev, stateful socket). You are paying the abstraction tax *and*
  inheriting the latency-coupling risk. Three plain threads + channels (the
  rejected alternative in §5 Risk 2) would deliver the same domain isolation with
  fewer foot-guns. The proposal names this and then talks itself out of it on
  grounds of "architectural purity the philosophy demands" — that is choosing
  purity over the user's actual context.
- **Per-domain in-process supervised restart with `JoinSet` + `catch_unwind` +
  `DomainFailed` events + degraded badges** (§1.5) is real engineering for a tool
  that systemd already restarts in 2 seconds. §5 Risk 1 then admits a libX11
  segfault "takes everyone down, period" — so the elaborate in-process
  supervision does **not** deliver the hard isolation it costs effort to build,
  because the genuinely-likely crash (C FFI into X11) bypasses it. This is the
  worst trade in the doc: real complexity, illusory isolation. For a couch tool,
  "let systemd restart the whole binary" is both simpler and *more* honest about
  the actual failure modes.
- **`Type=notify` + `WatchdogSec` + `sd_notify` heartbeats** (§1.1, §1.5) is
  production-daemon hygiene that is fine but firmly in "nice to have." Not wrong;
  just worth recognizing as polish, not architecture.

What *is* genuinely worth it: the cargo-workspace split with the CI-enforced
"siblings can't depend on siblings" rule (§2.1), and the pure `menu-state` machine
(§2.4, modulo the §1.1/§1.2 leaks). Those two give 80% of the modularity payoff
for 20% of the complexity. The bus, the reactor, and the in-process supervisor are
the over-built 80%-of-the-effort tail.

---

## 4. What's under-specified or won't work against gamescope / Steam Input

- **`set_active(true)` as "grab input, hold focus" is hand-waved and may not
  satisfy criterion 3.** The proposal redefines criterion 3's "grabs input and
  holds focus" as a *logical* capture: the `input` crate just stops forwarding nav
  keys to nobody (it never forwarded them anyway) while the menu reads them.
  But the SPEC's input model is **Steam Input emits keys via a virtual keyboard**.
  Those keystrokes go into the **focused window** (the game) too, unless something
  stops them. The proposal asserts "the game never lost real focus, so we release
  trivially" — but it never explains what prevents the menu's nav keystrokes
  (d-pad→arrows, A→Enter, B→Esc per SPEC) from *also* reaching the game while the
  menu is open. If the action-layer remaps the pad to arrow keys and those arrows
  land in Overwatch *and* the menu, that is a real input-bleed bug. The SPEC's
  design leans on the Steam Input **action layer** to do the mediation (only emit
  the nav keys while the layer is active), but the proposal's `set_active` is on
  the *wrong side* of the boundary — it is in our daemon, when the actual gate is
  in the Steam Input template. This is the single most important mechanical
  question in the whole tool and the architecture is silent on it beyond a
  reassuring sentence. **Under-specified, and possibly wrong about where the
  mediation happens.**
- **The chord-vs-nav key collision is not addressed.** SPEC notes gamescope masks
  left-Windows and says "use unmasked keys." The proposal references this once but
  never reconciles it with the fact that the nav keys (arrows/Enter/Esc) are
  *also* keys the game sees. Which keys are the "signal" keys, and how does the
  daemon distinguish a chord-signal key from a game's legitimate use of that same
  key when the menu is closed? Not specified.
- **External-overlay window is "created once, shown/hidden."** §1.3/§2.3 say the
  X11 override-redirect overlay window is never destroyed, just `set_visible`.
  Against gamescope this is plausible but unproven for the *menu* (discover-overlay
  proves the always-on activity overlay path, not an interactive, input-capturing
  menu surface). The proposal cites discover-overlay as proof, but discover-overlay
  is a passive read-only overlay — it never grabs input or hosts an interactive
  selector. The hardest part of criterion 3+4 (an *interactive* overlay that the
  controller drives, over a running game, in gamescope) has **no cited precedent**
  and is treated as solved-by-analogy. It is not.
- **`render()` is synchronous on the async reactor.** x11rb round-trips are
  blocking; the proposal acknowledges `spawn_blocking` but the `OverlayRenderer`
  trait (§2.3) is **not async** (`fn render(&mut self, vm) -> Result<...>`), so
  calling it on the reactor thread blocks the single event loop the whole design
  hinges on. Either the trait is wrong (should be async / on its own thread) or
  the reactor stalls during paint. Another trait that will change on contact.

---

## 5. Smaller but real

- **`couchcordd install` writing a udev rule** (§1.4) needs root; a `--user`
  daemon's subcommand cannot drop a file in `/etc/udev/rules.d` without
  privilege escalation. The install story silently crosses a privilege boundary
  the hardening block (`NoNewPrivileges=true`) elsewhere brags about. Under-specified.
- **`couchcordd doctor` checking "overlay atom supported?"** is good and should be
  promoted to a hard gate before any other work — it is the cheapest possible
  de-risk of criterion 1 and is currently buried as a footnote.
- **Config hot-reload via `watch()` returning a `BoxStream<Config>`** (§2.3) is
  more machinery for a tool whose config (`client_id`, theme, anchor) changes
  approximately never at runtime. Load-on-start is enough; hot-reload is polish
  dressed as a boundary.

---

## Top 3 changes that would most improve it

1. **Move the voice-channel filter into `discord-rpc` and define the asset
   (icon/avatar) pipeline explicitly.** Filtering is mechanism: `discord-rpc`
   should hand `menu-state` only voice channels, keeping the "pure brain" free of
   Discord taxonomy and eliminating the §1.1 cross-crate ripple. In the same pass,
   add a real home (a trait + crate, or an explicit "no icons" decision) for the
   CDN-fetched guild/user images that criteria 4 and 7 require — today they have
   none.

2. **Spike the two unproven domains FIRST and do not lock `RpcClient` /
   `InputSource` until they pass — and resolve where input mediation lives.**
   Reverse the implicit ordering: prove `SELECT_VOICE_CHANNEL` end-to-end and the
   Steam-Input-layer→daemon path on real hardware before designing their traits
   (the proposal's own Appendix A says this; make it the rule, not a hedge). Crucially,
   settle the criterion-3 question: the input gating belongs in the **Steam Input
   action layer** (only emit nav keys while the layer is active), not in the
   daemon's `set_active` — document this so the daemon isn't expected to prevent
   input-bleed it cannot prevent. Fix the per-channel (not per-guild) subscription
   granularity while you're there.

3. **Cut the in-process complexity to match a single-user couch tool: drop the
   per-domain supervised-restart machinery and the dual-channel bus+reactor in
   favor of plain threads + channels, and let systemd own crash recovery.** Keep
   the two parts that actually deliver the user's priority — the CI-enforced
   workspace dependency rule (§2.1) and the pure `menu-state` machine (§2.4) — and
   delete the §1.5 in-process supervisor (it cannot survive the libX11 segfault it
   exists to survive) and the latency-coupling single reactor (§5 Risk 2's own
   admission). Make `OverlayRenderer::render` async or thread-owned so a blocking
   X11 paint can never wedge the loop.
