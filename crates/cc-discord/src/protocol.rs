//! Pure Discord local-RPC protocol: framing, command building, parsing, filter.
//! No IO — every function here is a value→value transform, fully unit-tested.

use cc_core::{
    AssetHash, ChannelId, ClientId, Guild, GuildId, RpcError, UserId, VoiceChannel, VoiceKind,
};
use serde_json::{json, Value};

/// RPC IPC opcodes (4-byte LE header field 1).
pub const OP_HANDSHAKE: u32 = 0;
pub const OP_FRAME: u32 = 1;
pub const OP_CLOSE: u32 = 2;
pub const OP_PING: u32 = 3;
pub const OP_PONG: u32 = 4;

/// Encode one IPC frame: `u32 LE opcode | u32 LE length | json payload`.
pub fn frame(op: u32, payload: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(payload).expect("Value always serializes");
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Try to read one frame from the front of `buf`. Returns `Ok(None)` if the
/// buffer doesn't yet hold a complete frame (the caller reads more), or
/// `Ok(Some((op, value, consumed)))` with how many bytes to drain.
pub fn read_frame(buf: &[u8]) -> Result<Option<(u32, Value, usize)>, RpcError> {
    if buf.len() < 8 {
        return Ok(None);
    }
    let op = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
    if buf.len() < 8 + len {
        return Ok(None);
    }
    let value: Value =
        serde_json::from_slice(&buf[8..8 + len]).map_err(|e| RpcError::new(format!("json: {e}")))?;
    Ok(Some((op, value, 8 + len)))
}

// ---- command builders -----------------------------------------------------

/// Opening handshake (op 0). Identifies the registered application.
pub fn handshake(client_id: ClientId) -> Vec<u8> {
    frame(OP_HANDSHAKE, &json!({ "v": 1, "client_id": client_id.to_string() }))
}

fn command(cmd: &str, args: Value, nonce: &str) -> Vec<u8> {
    frame(OP_FRAME, &json!({ "cmd": cmd, "args": args, "nonce": nonce }))
}

pub fn get_guilds(nonce: &str) -> Vec<u8> {
    command("GET_GUILDS", json!({}), nonce)
}

pub fn get_channels(guild: GuildId, nonce: &str) -> Vec<u8> {
    command("GET_CHANNELS", json!({ "guild_id": guild.to_string() }), nonce)
}

/// Join (`Some`) or leave (`None` → `channel_id: null`) a voice channel.
pub fn select_voice(channel: Option<ChannelId>, nonce: &str) -> Vec<u8> {
    let id = channel.map(|c| Value::String(c.to_string())).unwrap_or(Value::Null);
    command("SELECT_VOICE_CHANNEL", json!({ "channel_id": id, "force": true }), nonce)
}

/// Subscribe to one voice event type for a specific channel.
pub fn subscribe_voice_events(channel: ChannelId, evt: &str, nonce: &str) -> Vec<u8> {
    frame(
        OP_FRAME,
        &json!({ "cmd": "SUBSCRIBE", "evt": evt, "args": { "channel_id": channel.to_string() }, "nonce": nonce }),
    )
}

// ---- response / event parsers ---------------------------------------------

/// Parse a `GET_GUILDS` response's `data` object into domain guilds.
pub fn parse_guilds(data: &Value) -> Vec<Guild> {
    data.get("guilds")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|g| {
                    let id = g.get("id")?.as_str()?.parse::<u64>().ok()?;
                    let name = g.get("name")?.as_str()?.to_string();
                    let icon = g.get("icon").and_then(Value::as_str).map(|s| AssetHash(s.to_string()));
                    Some(Guild { id: GuildId(id), name, icon })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `GET_CHANNELS` response's `data` object, keeping ONLY voice channels
/// whose kind is in `accept`. This is the one place "what is a voice channel"
/// lives — text channels never cross this boundary.
pub fn parse_voice_channels(data: &Value, accept: &[VoiceKind]) -> Vec<VoiceChannel> {
    data.get("channels")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let ty = c.get("type")?.as_u64()? as u8;
                    let kind = VoiceKind::from_discord_type(ty)?; // drops non-voice
                    if !accept.contains(&kind) {
                        return None; // config-level filter (e.g. Stage disabled)
                    }
                    let id = c.get("id")?.as_str()?.parse::<u64>().ok()?;
                    let name = c.get("name")?.as_str()?.to_string();
                    Some(VoiceChannel { id: ChannelId(id), name, kind })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `SPEAKING_START` / `SPEAKING_STOP` dispatch into `(channel, user, speaking)`.
pub fn parse_speaking(evt: &str, data: &Value) -> Option<(ChannelId, UserId, bool)> {
    let speaking = match evt {
        "SPEAKING_START" => true,
        "SPEAKING_STOP" => false,
        _ => return None,
    };
    let ch = data.get("channel_id")?.as_str()?.parse::<u64>().ok()?;
    let user = data.get("user_id")?.as_str()?.parse::<u64>().ok()?;
    Some((ChannelId(ch), UserId(user), speaking))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips() {
        let payload = json!({ "cmd": "GET_GUILDS", "nonce": "n1" });
        let bytes = frame(OP_FRAME, &payload);
        let (op, val, consumed) = read_frame(&bytes).unwrap().unwrap();
        assert_eq!(op, OP_FRAME);
        assert_eq!(val, payload);
        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn read_frame_needs_full_buffer() {
        let bytes = frame(OP_FRAME, &json!({ "x": 1 }));
        assert!(read_frame(&bytes[..4]).unwrap().is_none(), "header incomplete");
        assert!(read_frame(&bytes[..bytes.len() - 1]).unwrap().is_none(), "body incomplete");
        assert!(read_frame(&bytes).unwrap().is_some(), "complete");
    }

    #[test]
    fn handshake_carries_client_id_as_string() {
        let bytes = handshake(ClientId(1514871580591919246));
        let (op, val, _) = read_frame(&bytes).unwrap().unwrap();
        assert_eq!(op, OP_HANDSHAKE);
        assert_eq!(val["v"], 1);
        assert_eq!(val["client_id"], "1514871580591919246"); // string, not number
    }

    #[test]
    fn select_voice_join_vs_leave() {
        // join → channel_id is the id string
        let (_, join, _) = read_frame(&select_voice(Some(ChannelId(42)), "n")).unwrap().unwrap();
        assert_eq!(join["args"]["channel_id"], "42");
        // leave → channel_id is null
        let (_, leave, _) = read_frame(&select_voice(None, "n")).unwrap().unwrap();
        assert_eq!(leave["args"]["channel_id"], Value::Null);
        assert_eq!(leave["cmd"], "SELECT_VOICE_CHANNEL");
    }

    #[test]
    fn get_channels_carries_guild_id() {
        let (_, v, _) = read_frame(&get_channels(GuildId(7), "n")).unwrap().unwrap();
        assert_eq!(v["cmd"], "GET_CHANNELS");
        assert_eq!(v["args"]["guild_id"], "7");
    }

    #[test]
    fn parse_guilds_extracts_id_name_icon() {
        let data = json!({ "guilds": [
            { "id": "1", "name": "Friends", "icon": "abc123" },
            { "id": "2", "name": "Work" }
        ]});
        let g = parse_guilds(&data);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "Friends");
        assert_eq!(g[0].icon, Some(AssetHash("abc123".into())));
        assert_eq!(g[1].icon, None);
    }

    #[test]
    fn voice_filter_drops_text_keeps_voice_and_stage() {
        let data = json!({ "channels": [
            { "id": "10", "name": "general",   "type": 0 },   // text  → drop
            { "id": "11", "name": "announce",  "type": 5 },   // news  → drop
            { "id": "12", "name": "Voice",     "type": 2 },   // voice → keep
            { "id": "13", "name": "Stage",     "type": 13 },  // stage → keep
            { "id": "14", "name": "category",  "type": 4 },   // cat   → drop
        ]});
        let accept = [VoiceKind::Guild, VoiceKind::Stage];
        let chans = parse_voice_channels(&data, &accept);
        assert_eq!(chans.len(), 2);
        assert_eq!(chans[0].id, ChannelId(12));
        assert_eq!(chans[0].kind, VoiceKind::Guild);
        assert_eq!(chans[1].kind, VoiceKind::Stage);
    }

    #[test]
    fn voice_filter_respects_config_disabling_stage() {
        let data = json!({ "channels": [
            { "id": "12", "name": "Voice", "type": 2 },
            { "id": "13", "name": "Stage", "type": 13 },
        ]});
        let chans = parse_voice_channels(&data, &[VoiceKind::Guild]); // stage disabled
        assert_eq!(chans.len(), 1);
        assert_eq!(chans[0].kind, VoiceKind::Guild);
    }

    #[test]
    fn parse_speaking_start_stop() {
        let data = json!({ "channel_id": "10", "user_id": "100" });
        assert_eq!(parse_speaking("SPEAKING_START", &data), Some((ChannelId(10), UserId(100), true)));
        assert_eq!(parse_speaking("SPEAKING_STOP", &data), Some((ChannelId(10), UserId(100), false)));
        assert_eq!(parse_speaking("VOICE_STATE_UPDATE", &data), None);
    }
}
