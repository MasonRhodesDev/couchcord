//! couchcord daemon.
//!
//! Phase 0 only implements the `doctor` subcommand — the de-risk gate that
//! probes the three load-bearing assumptions of the whole design before any
//! domain code is built on top of them:
//!   1. the Discord local RPC socket is reachable,
//!   2. the Steam Input virtual keyboard is discoverable (and our user can read
//!      input devices — the `input`-group install decision),
//!   3. a gamescope nested-X display exists and advertises the
//!      `GAMESCOPE_EXTERNAL_OVERLAY` atom we render through.

mod doctor;
mod reactor; // composition-root reactor (generic + mock-tested; live wiring is Phase 5)

use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("doctor") => doctor::run(),
        Some("run") => {
            // The live event loop needs the Phase 3/4 boundary impls (evdev
            // InputSource, X11 OverlayRenderer, CDN AssetStore). The reactor
            // that consumes them (`reactor::Dispatcher`) is built and tested;
            // wiring it to real impls is Phase 5.
            eprintln!("couchcordd run: not yet wired — pending Phase 3/4 boundary impls.");
            eprintln!("the reactor is implemented and mock-tested; run `couchcordd doctor`.");
            ExitCode::from(3)
        }
        Some(other) => {
            eprintln!("couchcordd: unknown subcommand {other:?}\nusage: couchcordd <doctor|run>");
            ExitCode::from(2)
        }
        None => {
            eprintln!("usage: couchcordd <doctor|run>");
            ExitCode::from(2)
        }
    }
}
