# couchcord

Controller-driven Discord **voice control + activity overlay** for a gamescope
Big Picture / game-mode session — browse and join voice channels, leave, and see
who's talking, from the couch, while a game runs, **without Discord ever being a
focus-stealing window**.

- **What & why:** [`SPEC.md`](SPEC.md)
- **Architecture (canonical):** [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) —
  synthesized by an adversarial design panel; proposals/critiques in
  [`docs/panel/`](docs/panel).

## Status

| Phase | What | State |
|---|---|---|
| 0 | `couchcordd doctor` de-risk gate | ✅ done — gamescope external-overlay atom **verified** |
| 1 | Pure core + all crate logic, unit-tested | ✅ done |
| 2 | `cc-discord` RPC client (async actor) | ✅ code + **mock-socket test**; live auth round-trip pending Discord + app |
| 5a | Composition reactor (`Dispatcher`) | ✅ code + **mock integration test** (full flow); live wiring pending P3/P4 impls |
| 3 | Steam-Input keyboard flow mid-game | ⏳ needs controller + game session |
| 4 | X11 overlay window + CDN image fetch | ⏳ needs the gamescope session |

**51 tests, no hardware required.** The two remaining phases are the genuine
hardware boundaries the architecture says to design *after* a live spike — they
need you at the desk with the controller and a game session, and Discord running
for the auth round-trip.

## Crates (cargo workspace, compile-isolated)

Domain crates may depend **only** on `cc-core`, never on each other — the
modularity is mechanical, not aspirational.

| Crate | Responsibility | Phase 1 (tested) | Deferred IO |
|---|---|---|---|
| `cc-core` | shared types, domain enums, boundary traits, `Scene` | types/traits | — |
| `cc-config` | TOML load/validate + `ArcSwap` ConfigSource | parse, defaults, snapshot | — |
| `cc-discord` | local-RPC framing, parsing, **voice filter** | protocol + filter | socket + OAuth (P2) |
| `cc-menu` | the pure app state machine | **full flow** | — |
| `cc-render` | gamescope external-overlay | **8-anchor geometry** | X11 window (P4) |
| `cc-assets` | Discord CDN hash → image | URL + cache | HTTP fetch (P4) |
| `cc-input` | Steam virtual-kbd → intents | classify + keymap | evdev grab (P3) |
| `couchcordd` | daemon: `doctor` now, composition root later | doctor | reactor (P5) |

## Build & test

```sh
cargo test --workspace      # 48 tests, no hardware needed
cargo run -p couchcordd -- doctor   # re-run inside a game-mode session to verify P2/P3 assumptions
```
