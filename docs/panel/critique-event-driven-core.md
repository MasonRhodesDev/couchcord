# Critique — "A Typed Event Bus as the Unit of Modularity"

Reviewer stance: hard-nosed, skeptical. I am grading this proposal against the
user's *stated* top priorities (modularity, clean boundaries, **ease of
upgrades**, **domain isolation**) and against the spec's **hard constraints**
(official local RPC only, gamescope external-overlay rendering, Steam-Input
keyboard mediation, Rust, never a focus-stack window, single-user couch tool).

The proposal is well-written and self-aware (its section 5 pre-empts three of my
objections). That earns it credit, but self-awareness is not mitigation, and
several of the "honest risks" are load-bearing problems dressed up as footnotes.
Below I attack the specific claims.

---

## A. Where the boundaries are actually leaky

### A1. The broadcast bus violates the central invariant it sells

The headline claim (lines 8–11, 480–483) is: *"No module holds a handle to
another module... the message enum is the only contract... the only way to
create coupling is to change the message enum."* That is the entire pitch for
isolation and easy upgrades.

A single `tokio::broadcast` bus (line 62, 240) **breaks this invariant in the
runtime even though it preserves it in the type system.** Every module receives
every `Msg` and must `match` it. That creates three concrete leaks the
dependency-graph story papers over:

- **Temporal/ordering coupling.** `cc-render` cannot paint `JoinedVoice` until
  `cc-menu` has consumed the `DiscordEvent` and emitted a `RenderIntent`. The
  modules are decoupled in the type graph but tightly coupled in a *delivery
  order* that nothing in the contract expresses or enforces. "Upgrade `cc-menu`"
  can silently change overlay latency or ordering with zero contract diff — the
  exact rippling the proposal claims is impossible.
- **Fan-out coupling.** Section 5.1 (lines 542–552) admits `SpeakingChanged`
  fans out to modules that don't care and a slow `cc-render` can `Lagged`-drop a
  `JoinedVoice`. The proposal's own example — *"the overlay state silently
  desyncs"* — is a cross-domain failure caused purely by the shared-bus choice.
  That is a leaky boundary by definition: Discord's event rate now affects
  render correctness through a channel neither domain names.
- **The mitigation re-introduces the coupling structurally.** The fix offered
  (lines 547–551) is per-module bounded inboxes with *per-message-type* overflow
  policy: "coalesce render intents... never coalesce `DiscordCommand`s." That
  policy is domain knowledge about what each message *means*, living in the bus.
  The moment the bus must know that `RenderIntent` is coalescable and
  `DiscordCommand` is not, **the bus is no longer "pure plumbing" (line 203) and
  is no longer swappable without understanding every domain.** The seam the whole
  design rests on (line 510, "swap broadcast for a priority queue without
  touching modules") is already false by section 5.

**Verdict on A1:** the "one enum is the only coupling" claim is true at compile
time and false at runtime. For a *modularity-first* brief, runtime coupling that
the contract can't see is the worst kind, because it is invisible in exactly the
review artifact (the `cc-contract` diff) the proposal tells you to trust.

### A2. `cc-menu` is a god-module that quietly owns three domains

Lines 216–219 / 368–382: `cc-menu` "is the only module that understands what the
app does." Look at what flows *through* it: every Discord event must pass through
the menu to become a render intent (lines 414–415, 423–428). The overlay roster
— pure Discord voice-state data — is held as menu state (`Connected` carries
roster; line 381 "tracked independently"). So:

- A change to *what voice activity looks like* (a render concern) or *how
  speaking maps to roster* (a Discord concern) lands in `cc-menu`. The blast-
  radius table (line 521) lists "new menu flow / restyle → cc-menu" as if menu
  and style were one domain, but **restyling the Steam-themed menu is a
  `cc-render` concern** per lines 461–464, and *also* a `cc-menu` concern because
  `MenuView` is constructed there. Which crate owns "make the channel list two
  columns"? The proposal answers both, depending on the paragraph. That is a
  boundary that isn't actually drawn.
- The overlay is supposed to keep rendering *with the menu closed and input
  released* (lines 428, 381). But the producer of `Render(Overlay{..})` is
  `cc-menu` (line 426). So the menu module is on the hot path of the
  always-on HUD even when there is no menu. The "pure logic, richest test suite,
  safest to iterate" crate (line 507) is also the crate you cannot touch without
  risking the live overlay. Those two properties are in tension and the proposal
  asserts only the flattering one.

A cleaner cut — a `cc-overlay-model` that derives roster directly from
`DiscordEvent` independent of menu state — is never considered, even though the
spec treats the overlay (criterion 7) and the menu (criteria 5–6) as separate
features.

### A3. `InputControl` grab/release is a correctness-critical control loop run over a lossy bus

Criterion 3 (grab input on chord, release on dismiss, *return focus to the
game*) is a **safety property**: if a `ReleaseNavigation` is dropped, the
controller stays remapped and the user's game is unplayable until they find a
keyboard. Yet `InputControl` (lines 285, 460) rides the same `broadcast` bus
that section 5.1 admits can drop messages under lag. There is **no
acknowledgement, no idempotent re-assert, no timeout-driven safe-release**
specified. For a couch tool whose entire reason to exist is "never trap the
user," shipping the grab/release handshake over a fire-and-forget lossy channel
with no failsafe is the single most dangerous under-specification here. (See C2
for why this is also a gamescope-specific landmine.)

---

## B. Over-engineering for a one-user couch tool

The spec explicitly says modularity *outranks* simplicity (SPEC lines 67–71), so
I am not going to swing the "it's just a couch tool, keep it simple" hammer at
everything. But "modularity-first" is not a licence for ceremony that buys *no*
isolation. These items add cost without advancing any of the four priorities:

### B1. Nine crates is boundary theater for several of them

`cc-bus`, `cc-supervisor`, and `cc-contract` as **separate crates** (lines
182–191) is justified ("a module literally cannot reach into another's
internals," line 174). Fine. But:

- **`cc-config` as its own crate publishing `ConfigChanged(Config)` on the bus**
  (lines 213, 273) means the *entire* `Config` struct is a contract type, and
  every config field change is a `cc-contract` change — i.e. the "single visible
  upgrade surface" gets churned by trivia like adding a theme color. The proposal
  brags (line 202) that "the diff that matters is always visible in one small
  crate"; routing config through the contract *guarantees that crate is not
  small and not stable.* Config is genuinely a single-user local file; a plain
  `Arc<ArcSwap<Config>>` read by whoever needs it would isolate config churn
  *better* than putting it on the spine.
- **`AnchorCycle` / anchor persistence via `ConfigChanged`** (lines 433–435):
  cycling the overlay corner emits a "ConfigChanged persistence intent" on the
  bus. So moving the HUD to another corner is modeled as a global config-domain
  event broadcast to every module. That is a lot of architecture for "remember
  which of 8 corners." It also creates a write-back loop (`cc-menu` emits
  `ConfigChanged`, `cc-config` presumably persists and re-emits?) that is left
  unspecified and is a classic event-bus feedback footgun.

### B2. Two layers of supervision, one of which the proposal admits doesn't work

Lines 114–124 and 565–576: there is a systemd layer *and* an in-process
`cc-supervisor` that restarts individual module tasks. Then section 5.3 honestly
concedes the in-process supervisor "cannot save us from `abort`-on-panic in FFI
or a deadlocked blocking thread" — and the two highest-risk modules (`cc-render`
X11 FFI on a blocking thread, `cc-input` uinput FFI) are *exactly* the ones whose
failure modes the in-process supervisor can't catch. So the per-module
supervisor reliably handles... `cc-menu` and `cc-discord`, the two modules least
likely to hard-crash. For a single-user tool where systemd already gives
`Restart=on-failure` + `WatchdogSec`, the in-process supervisor with backoff and
`ModuleHealth` events is **machinery whose main value (FFI fault containment) it
explicitly cannot deliver.** Keep the watchdog; the second supervision layer is
mostly ceremony until the `overlay-ipc` split exists, and the proposal defers
that split to step 8 "(insurance, deferred)" (line 589).

### B3. `CONTRACT_VERSION` (line 530) is pure cargo-cult for an in-process single binary

Every module is compiled from the same `cc-contract` source in the same `cargo
build`. There is no skew possible across an in-process boundary — the compiler
*is* the version check. A runtime `CONTRACT_VERSION` logged at boot guards a
boundary that does not exist in the default architecture. It would only matter
after the `overlay-ipc` process split, and even then only across that one
socket. Logging it as if it protects the whole system is misleading.

---

## C. What won't actually work against gamescope / Steam Input / the spec

This is where the proposal is thinnest. It is strong on Rust crate hygiene and
weak on the three hard, physical constraints that the spec spent its entire "Key
technical findings" section de-risking.

### C1. `WantedBy=graphical-session.target` does not reliably mean "the gamescope X overlay is ready"

Lines 76–94 lean hard on systemd: import `DISPLAY`/`XDG_RUNTIME_DIR` via
`systemctl --user import-environment`, order `After=graphical-session.target`.
The spec's rendering path is a **gamescope external-overlay X11 window on the
gamescope-nested X display** (SPEC lines 52–56). Problems the proposal does not
address:

- In a Steam Big Picture / gamescope session, the gamescope X server the overlay
  must attach to is **not** `graphical-session.target`'s display in general; it
  is gamescope's nested X server whose `DISPLAY` (and the `GAMESCOPE_EXTERNAL_-
  OVERLAY` atom owner) appears **when gamescope starts**, which may be after the
  target is "reached." `import-environment` is a one-shot snapshot; if the daemon
  starts before gamescope publishes its display, the `render-sink` adapter
  attaches to the wrong (or no) X server. There is no specified retry/rediscovery
  of the gamescope display, only "the unit imports... so paths resolve" (line
  91–94) stated as if settled.
- Conversely, if the daemon is *too* decoupled from the session it is supposed to
  draw into, the env-capture story gets fragile across session restarts (Steam
  re-launching gamescope). The proposal picks systemd ownership specifically to
  stay *out* of Steam's process tree (lines 82–86, a correct instinct for
  criterion 1) but then needs intimate, timing-sensitive knowledge of Steam's
  gamescope display — and never reconciles the two. **This is the proposal's
  biggest unaddressed feasibility gap**, and it sits squarely on the rendering
  hard constraint.

### C2. The Steam-Input "release back to game" handshake is under-specified against the masked-key gotcha

The spec calls out a specific, hard-won gotcha: **gamescope masks the
left-Windows key; use unmasked keys** (SPEC lines 62–63). The proposal's input
story (lines 130–133, 200–208) says `cc-input` reads the virtual keyboard and
maps keys to semantic events, and the chord emits "a signal key." Nowhere does it
engage with *which* keys survive gamescope masking, how the Steam Input action
layer is torn down on release, or — critically — what happens to the **remapped
controller** if `cc-menu`'s `ReleaseNavigation` never produces a corresponding
Steam-Input layer pop. The architecture models input as a clean
`OpenChord/Confirm/Dismiss` enum (lines 279–283), but the *real* control problem
is a stateful two-sided handshake with Steam Input's action-layer stack, which
lives **outside** the daemon entirely (in the Steam Input template, SPEC lines
77, 83–84). `cc-input` can read keys, but it has **no channel to command Steam
Input to pop the action layer** — that is driven by the controller template, not
by uinput reads. So `InputControl(ReleaseNavigation)` (line 285) can change how
the *daemon interprets* keys, but the proposal never shows how the controller
*stops emitting remapped keys*. This is a genuine boundary the architecture
doesn't have a module for: the Steam Input layer state is a fourth external edge,
and it is missing from the table at lines 143–148 and the edges list at 130–139.

### C3. "Domain verbs not RPC verbs" oversells RPC-change isolation given a fixed whitelisted command set

Lines 287–298, 330–334 make the marquee isolation claim: the menu says
`JoinVoice`, never `SELECT_VOICE_CHANNEL`, so a Discord RPC change touches only
`cc-discord`. True *as far as it goes* — but the spec's hard constraint is that
the command set is **fixed and whitelist-gated** (SPEC lines 38–46): there is no
soundboard, leave is `SELECT_VOICE_CHANNEL{null}`, etc. The set of plausible
"upgrades" here is therefore tiny and well-bounded. The proposal spends its
strongest isolation argument defending against a category of change (Discord
reworking its RPC verbs) that is **low-probability for a frozen official
surface**, while spending much less rigor on the changes that are *actually*
likely for this tool: a different overlay anchor scheme, a restyle, a new input
chord, reconnect/auth-token-refresh behavior. The auth/reconnect path in
particular (`AUTHORIZE`→token→`AUTHENTICATE`, line 135; "reconnect," line 211) is
the most failure-prone real-world Discord-domain behavior and gets one word,
while verb-renaming gets a whole subsection (4.1 first bullet). Priorities
inverted relative to where churn will actually come from.

### C4. Under-specified: cache coherence and the "list already cached" assumption

Lines 397–398: browsing servers emits "No Discord traffic; list already cached
from `DiscordEvent::Guilds`." But guild/channel membership, who's in a voice
channel, and permissions change server-side at any time. The proposal never
specifies cache invalidation or refresh-on-open. For a tool you open *mid-game,
occasionally,* the cached `GuildList` could be stale for hours. This is a
small thing, but it is exactly the kind of correctness detail a "build plan, not
a survey" (line 13) should pin down, and it interacts with C1's reconnect gap.

---

## D. Things the proposal gets right (credit where due)

- **systemd-user ownership for criterion 1** (lines 82–86) is the correct
  structural answer to the focus-trap problem and is argued well. Staying out of
  Steam's process tree is exactly right; my C1 objection is about *display
  discovery*, not about the ownership decision.
- **Domain-typed sub-enums** (`DiscordCommand`, `InputEvent`) instead of one flat
  blob (line 562 acknowledges the fat-enum temptation) is good taste.
- **`cc-menu` as the only syscall-free, `MockBus`-testable module** (lines
  336–366) is a genuinely valuable property and the right place to concentrate
  tests — *if* A2's god-module creep is contained.
- **Rejecting internal multiprocess/D-Bus** (section 0, Appendix B) is the
  correct call for a single-user tool and is well-justified; the typed enum *is*
  a stronger internal contract than wire bytes.
- **Section 5 honesty.** Pre-stating the broadcast-lag, fat-enum, and
  fault-isolation risks is more than most proposals do. My complaint is that two
  of the three are downgraded to "risks" when they are actually unsolved design
  holes (A1, B2).

---

## Top 3 changes that would most improve it

1. **Fix the bus-vs-isolation contradiction directly: replace the single
   `broadcast` bus with explicit typed channels per producer→consumer edge (or a
   router that owns the per-message overflow policy as a first-class, tested
   component), and make the grab/release control path acknowledged and
   self-healing.** This is the #1 change because the entire proposal's value
   proposition — "the message enum is the only coupling, the bus is pure
   plumbing" — is contradicted by its own section 5.1 the moment lag and
   coalescing enter. Either the bus is dumb (then drops break cross-domain
   correctness, see A1/A3) or it is smart (then it knows every domain and isn't
   swappable). Pick channels with named back-pressure semantics, and give
   `InputControl(ReleaseNavigation)` an ack + watchdog-driven safe-release so a
   dropped message can never leave the user's controller remapped mid-game.

2. **Add the missing fourth external edge — Steam Input action-layer state — as a
   real module/boundary, and specify gamescope display discovery for the
   renderer.** The two hardest spec constraints (Steam-Input mediation, gamescope
   external-overlay) are the two the architecture under-models. Concretely:
   (a) define how the daemon coordinates the Steam Input layer pop on release
   (not just how it reads keys), since that state lives outside uinput (C2);
   (b) replace the one-shot `import-environment` story with explicit, retrying
   discovery of the gamescope nested-X display + `GAMESCOPE_EXTERNAL_OVERLAY`
   atom owner, tolerant of gamescope starting after the daemon (C1).

3. **Cut the ceremony that buys no isolation, and re-aim the isolation argument
   at where churn actually lives.** Take `Config` off the contract spine (use a
   shared `ArcSwap`, keep `cc-config` as a crate but not a `Msg` variant), drop
   `CONTRACT_VERSION` until the `overlay-ipc` split exists, and collapse the
   in-process per-module supervisor into systemd's watchdog until the X11 process
   split is real (B1–B3). Then redirect the freed rigor toward the Discord
   auth/reconnect/token-refresh and cache-staleness paths (C3/C4), which are the
   genuinely likely upgrades for a frozen-RPC couch tool — unlike the
   RPC-verb-rename scenario the proposal over-defends.
