# Setup (one-time)

## Discord application
- Application ID (client_id): `1514871580591919246`
  - Public, non-secret identifier (like discover-overlay's hardcoded id).
  - Owner = Mason, so RPC is usable without separate Discord approval.
  - Used by the RPC client to AUTHORIZE/AUTHENTICATE against the local
    `discord-ipc-0` socket.
- TODO during build: confirm SELECT_VOICE_CHANNEL works with this app on the
  account (the privileged-RPC live validation).

## Still pending
- Steam Input controller template (chord + menu action layer).
- Native Discord background-launch wiring in the game-mode session.
