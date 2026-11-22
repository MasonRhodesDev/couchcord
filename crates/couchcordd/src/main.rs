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
mod tenant;

use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("doctor") => doctor::run(),
        Some("run") => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "couchcordd=info,cc_discord=info".into()),
                )
                .init();
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("couchcordd: tokio runtime: {e}");
                    return ExitCode::from(1);
                }
            };
            match rt.block_on(reactor::run::run_live()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("couchcordd run: {e}");
                    ExitCode::from(1)
                }
            }
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
