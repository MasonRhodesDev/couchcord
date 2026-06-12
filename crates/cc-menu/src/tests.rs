//! Full-flow tests for the pure `MenuEngine`. Every test drives the engine with
//! plain enums and asserts on the emitted `Step` (commands, controls, scene) —
//! no IO, no mocks beyond data.

use super::*;
use cc_core::{ClientId, DisconnectReason, UserId};

fn cfg() -> Config {
    Config {
        client_id: ClientId(1514871580591919246),
        anchor: Anchor::TopRight,
        voice_kinds: vec![VoiceKind::Guild, VoiceKind::Stage],
        theme: Default::default(),
    }
}

fn guild(id: u64, name: &str) -> Guild {
    Guild { id: GuildId(id), name: name.into(), icon: None }
}
fn vchan(id: u64, name: &str, kind: VoiceKind) -> VoiceChannel {
    VoiceChannel { id: ChannelId(id), name: name.into(), kind }
}
fn member(id: u64, name: &str, speaking: bool) -> VoiceMember {
    VoiceMember {
        user: UserId(id),
        name: name.into(),
        avatar: None,
        speaking,
        muted: false,
        deafened: false,
    }
}

/// Build an engine already browsing a populated guild's voice channels.
fn engine_in_channels() -> MenuEngine {
    let mut e = MenuEngine::new(&cfg());
    e.on_input(InputIntent::Chord); // open → loading guilds
    e.on_discord(DiscordEvent::Guilds(vec![guild(1, "Friends"), guild(2, "Work")]));
    e.on_input(InputIntent::Confirm); // select guild 1 → loading channels
    e.on_discord(DiscordEvent::VoiceChannels {
        guild: GuildId(1),
        channels: vec![
            vchan(10, "General", VoiceKind::Guild),
            vchan(11, "Gaming", VoiceKind::Guild),
            vchan(12, "Town Hall", VoiceKind::Stage),
        ],
    });
    e
}

#[test]
fn starts_closed_and_blank() {
    let e = MenuEngine::new(&cfg());
    assert!(!e.menu_open());
    assert!(e.scene().is_blank());
}

#[test]
fn chord_opens_menu_grabs_and_requests_guilds() {
    let mut e = MenuEngine::new(&cfg());
    let step = e.on_input(InputIntent::Chord);
    assert_eq!(step.controls, vec![InputControl::Grab]);
    assert_eq!(step.cmds, vec![DiscordCommand::ListGuilds]);
    assert!(e.menu_open());
    // shows a loading menu, no overlay
    let m = step.scene.menu.unwrap();
    assert_eq!(m.title, "Servers");
    assert_eq!(m.rows[0].state, RowState::Loading);
    assert!(step.scene.overlay.is_none());
}

#[test]
fn guilds_event_populates_server_list() {
    let mut e = MenuEngine::new(&cfg());
    e.on_input(InputIntent::Chord);
    let step = e.on_discord(DiscordEvent::Guilds(vec![guild(1, "Friends"), guild(2, "Work")]));
    let m = step.scene.menu.unwrap();
    assert_eq!(m.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["Friends", "Work"]);
    assert_eq!(m.selected, 0);
}

#[test]
fn up_down_wraps_the_cursor() {
    let mut e = MenuEngine::new(&cfg());
    e.on_input(InputIntent::Chord);
    e.on_discord(DiscordEvent::Guilds(vec![guild(1, "A"), guild(2, "B"), guild(3, "C")]));
    assert_eq!(e.on_input(InputIntent::Down).scene.menu.unwrap().selected, 1);
    assert_eq!(e.on_input(InputIntent::Down).scene.menu.unwrap().selected, 2);
    assert_eq!(e.on_input(InputIntent::Down).scene.menu.unwrap().selected, 0); // wrap
    assert_eq!(e.on_input(InputIntent::Up).scene.menu.unwrap().selected, 2); // wrap back
}

#[test]
fn selecting_a_guild_requests_its_voice_channels() {
    let mut e = MenuEngine::new(&cfg());
    e.on_input(InputIntent::Chord);
    e.on_discord(DiscordEvent::Guilds(vec![guild(1, "Friends"), guild(2, "Work")]));
    e.on_input(InputIntent::Down); // select "Work" (id 2)
    let step = e.on_input(InputIntent::Confirm);
    assert_eq!(step.cmds, vec![DiscordCommand::ListVoiceChannels { guild: GuildId(2) }]);
    assert_eq!(step.scene.menu.unwrap().rows[0].state, RowState::Loading);
}

#[test]
fn channel_list_shows_only_voice_and_marks_stage() {
    let e = engine_in_channels();
    let m = e.scene().menu.unwrap();
    let labels: Vec<&str> = m.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["General", "Gaming", "🎙 Town Hall"]); // stage prefixed, all voice
}

#[test]
fn confirming_a_channel_joins_subscribes_releases_and_closes() {
    let mut e = engine_in_channels();
    e.on_input(InputIntent::Down); // cursor → "Gaming" (id 11)
    let step = e.on_input(InputIntent::Confirm);
    assert_eq!(
        step.cmds,
        vec![
            DiscordCommand::JoinVoice { channel: ChannelId(11) },
            DiscordCommand::SubscribeVoice { channel: ChannelId(11) },
        ]
    );
    assert_eq!(step.controls, vec![InputControl::Release]);
    assert!(!e.menu_open(), "menu closes on join");
    // overlay appears immediately (optimistic), roster empty until members arrive
    let ov = step.scene.overlay.unwrap();
    assert_eq!(ov.roster.channel_name, "Gaming");
    assert!(ov.roster.members.is_empty());
    assert!(step.scene.menu.is_none());
}

#[test]
fn joined_then_members_then_speaking_updates_overlay() {
    let mut e = engine_in_channels();
    e.on_input(InputIntent::Confirm); // join "General" (id 10)
    e.on_discord(DiscordEvent::JoinedVoice { channel: ChannelId(10) });
    e.on_discord(DiscordEvent::VoiceMembers {
        channel: ChannelId(10),
        members: vec![member(100, "mason", false), member(101, "cal", false)],
    });
    let step = e.on_discord(DiscordEvent::SpeakingChanged {
        channel: ChannelId(10),
        user: UserId(101),
        speaking: true,
    });
    let roster = step.scene.overlay.unwrap().roster;
    assert_eq!(roster.members.len(), 2);
    assert!(!roster.members[0].speaking);
    assert!(roster.members[1].speaking, "cal should be marked speaking");
}

#[test]
fn overlay_persists_with_menu_closed_then_reopened() {
    let mut e = engine_in_channels();
    e.on_input(InputIntent::Confirm); // join → menu closes, overlay live
    assert!(!e.menu_open());
    assert!(e.scene().overlay.is_some(), "overlay lives while menu closed");
    // reopen menu: overlay still present alongside the menu (independent layers)
    let step = e.on_input(InputIntent::Chord);
    assert!(step.scene.menu.is_some());
    assert!(step.scene.overlay.is_some());
}

#[test]
fn leave_via_action_row_emits_leave_and_clears_overlay() {
    let mut e = engine_in_channels();
    e.on_input(InputIntent::Confirm); // join id 10
    e.on_discord(DiscordEvent::JoinedVoice { channel: ChannelId(10) });
    // reopen, go back into the channel list for the guild
    e.on_input(InputIntent::Chord);
    e.on_discord(DiscordEvent::Guilds(vec![guild(1, "Friends")]));
    e.on_input(InputIntent::Confirm); // re-enter channels
    e.on_discord(DiscordEvent::VoiceChannels {
        guild: GuildId(1),
        channels: vec![vchan(10, "General", VoiceKind::Guild)],
    });
    // row 0 is now the "Leave voice" action row
    let m = e.scene().menu.unwrap();
    assert_eq!(m.rows[0].state, RowState::Action);
    let step = e.on_input(InputIntent::Confirm); // confirm Leave
    assert_eq!(
        step.cmds,
        vec![DiscordCommand::LeaveVoice, DiscordCommand::UnsubscribeVoice { channel: ChannelId(10) }]
    );
    e.on_discord(DiscordEvent::LeftVoice);
    assert!(e.scene().overlay.is_none(), "overlay clears after leaving");
}

#[test]
fn confirming_the_connected_channel_toggles_leave() {
    let mut e = engine_in_channels();
    e.on_input(InputIntent::Confirm); // join "General" id 10 (cursor 0)
    e.on_discord(DiscordEvent::JoinedVoice { channel: ChannelId(10) });
    e.on_input(InputIntent::Chord); // reopen
    e.on_discord(DiscordEvent::Guilds(vec![guild(1, "Friends")]));
    e.on_input(InputIntent::Confirm);
    e.on_discord(DiscordEvent::VoiceChannels {
        guild: GuildId(1),
        channels: vec![vchan(10, "General", VoiceKind::Guild)],
    });
    // rows: [Leave, General(active)]; move to General and confirm → leaves
    e.on_input(InputIntent::Down);
    let step = e.on_input(InputIntent::Confirm);
    assert!(step.cmds.contains(&DiscordCommand::LeaveVoice));
}

#[test]
fn back_navigates_channels_to_guilds_then_closes() {
    let mut e = engine_in_channels();
    let step = e.on_input(InputIntent::Back); // channels → guilds
    assert!(step.scene.menu.is_some());
    assert_eq!(step.scene.menu.unwrap().title, "Servers");
    let step = e.on_input(InputIntent::Back); // guilds → closed
    assert!(!e.menu_open());
    assert_eq!(step.controls, vec![InputControl::Release]);
}

#[test]
fn dismiss_closes_menu_but_keeps_overlay() {
    let mut e = engine_in_channels();
    e.on_input(InputIntent::Confirm); // join, menu closes
    e.on_input(InputIntent::Chord); // reopen
    let step = e.on_input(InputIntent::Dismiss);
    assert!(!e.menu_open());
    assert_eq!(step.controls, vec![InputControl::Release]);
    assert!(step.scene.overlay.is_some());
    assert!(step.scene.menu.is_none());
}

#[test]
fn anchor_cycle_only_moves_when_connected_and_walks_all_eight() {
    let mut e = MenuEngine::new(&cfg());
    e.on_input(InputIntent::Chord);
    // not connected → AnchorCycle is a no-op
    let before = e.anchor();
    e.on_input(InputIntent::AnchorCycle);
    assert_eq!(e.anchor(), before, "no anchor change without a connection");

    let mut e = engine_in_channels();
    e.on_input(InputIntent::Confirm); // connect
    let start = e.anchor();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..8 {
        seen.insert(e.anchor());
        e.on_input(InputIntent::AnchorCycle);
    }
    assert_eq!(seen.len(), 8, "cycles through all 8 anchors");
    assert_eq!(e.anchor(), start, "returns to start after 8");
}

#[test]
fn disconnected_clears_the_overlay() {
    let mut e = engine_in_channels();
    e.on_input(InputIntent::Confirm); // connect
    assert!(e.scene().overlay.is_some());
    let step = e.on_discord(DiscordEvent::Disconnected { reason: DisconnectReason::ClientNotRunning });
    assert!(step.scene.overlay.is_none(), "lost Discord → overlay gone");
}

#[test]
fn speaking_for_other_channel_is_ignored() {
    let mut e = engine_in_channels();
    e.on_input(InputIntent::Confirm); // join id 10
    e.on_discord(DiscordEvent::JoinedVoice { channel: ChannelId(10) });
    e.on_discord(DiscordEvent::VoiceMembers {
        channel: ChannelId(10),
        members: vec![member(100, "mason", false)],
    });
    // a speaking event for a DIFFERENT channel must not touch our roster
    let step = e.on_discord(DiscordEvent::SpeakingChanged {
        channel: ChannelId(999),
        user: UserId(100),
        speaking: true,
    });
    assert!(!step.scene.overlay.unwrap().roster.members[0].speaking);
}

#[test]
fn input_while_closed_is_inert() {
    let mut e = MenuEngine::new(&cfg());
    for i in [InputIntent::Up, InputIntent::Down, InputIntent::Confirm, InputIntent::Back] {
        let step = e.on_input(i);
        assert!(step.cmds.is_empty());
        assert!(step.controls.is_empty());
        assert!(step.scene.is_blank());
    }
}
