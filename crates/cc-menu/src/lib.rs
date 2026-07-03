//! `cc-menu` — the pure application brain.
//!
//! `MenuEngine` is a state machine with **zero IO and zero syscalls**: it folds
//! `InputIntent` / `DiscordEvent` / `Config` into a new state and a `Step`
//! (commands to issue, input-capture controls, and a `Scene` to draw). Because
//! it is pure, the entire app flow is unit-testable with plain enums — which is
//! exactly what this module's test suite does.
//!
//! Layer independence (ARCHITECTURE §2.2): the menu screen and the voice overlay
//! are tracked separately, so the roster HUD renders whenever connected,
//! regardless of whether the menu is open.

use cc_core::{
    Anchor, ChannelId, Config, DiscordCommand, DiscordEvent, Guild, GuildId, InputControl,
    InputIntent, MenuView, Overlay, Roster, Row, RowState, Scene, VoiceChannel, VoiceKind,
    VoiceMember,
};

/// The output of feeding one event into the engine.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// Discord verbs to issue (order matters).
    pub cmds: Vec<DiscordCommand>,
    /// Input-capture transitions (grab on open, release on close).
    pub controls: Vec<InputControl>,
    /// The complete next frame.
    pub scene: Scene,
}

/// Which menu screen is showing. `Closed` means the menu layer is hidden (the
/// overlay may still be live).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Screen {
    Closed,
    Guilds {
        cursor: usize,
        loading: bool,
    },
    Channels {
        guild: GuildId,
        cursor: usize,
        loading: bool,
    },
}

/// The live voice connection (the overlay's source of truth), independent of the
/// menu screen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Connection {
    channel: ChannelId,
    name: String,
    members: Vec<VoiceMember>,
}

/// The pure state machine.
pub struct MenuEngine {
    screen: Screen,
    guilds: Vec<Guild>,
    /// Voice channels of the guild currently being browsed.
    channels: Vec<VoiceChannel>,
    connection: Option<Connection>,
    anchor: Anchor,
}

impl MenuEngine {
    pub fn new(cfg: &Config) -> Self {
        MenuEngine {
            screen: Screen::Closed,
            guilds: Vec::new(),
            channels: Vec::new(),
            connection: None,
            anchor: cfg.anchor,
        }
    }

    /// Is the menu layer currently visible? (test/inspection helper)
    pub fn menu_open(&self) -> bool {
        !matches!(self.screen, Screen::Closed)
    }

    /// The current overlay anchor (test/inspection + the reactor persists this).
    pub fn anchor(&self) -> Anchor {
        self.anchor
    }

    // ---- inputs -----------------------------------------------------------

    pub fn on_input(&mut self, intent: InputIntent) -> Step {
        let mut cmds = Vec::new();
        let mut controls = Vec::new();
        match intent {
            InputIntent::Chord => {
                if matches!(self.screen, Screen::Closed) {
                    self.screen = Screen::Guilds {
                        cursor: 0,
                        loading: true,
                    };
                    controls.push(InputControl::Grab);
                    cmds.push(DiscordCommand::ListGuilds);
                } else {
                    self.close_menu(&mut controls);
                }
            }
            InputIntent::Dismiss => self.close_menu(&mut controls),
            InputIntent::Up => self.move_cursor(-1),
            InputIntent::Down => self.move_cursor(1),
            InputIntent::Confirm => self.confirm(&mut cmds, &mut controls),
            InputIntent::Back => self.back(&mut controls),
            InputIntent::Left => self.back(&mut controls),
            InputIntent::Right => self.confirm(&mut cmds, &mut controls),
            InputIntent::AnchorCycle if self.connection.is_some() => {
                self.anchor = self.anchor.next();
            }
            _ => {} // future intents: inert until handled
        }
        self.step(cmds, controls)
    }

    fn close_menu(&mut self, controls: &mut Vec<InputControl>) {
        if !matches!(self.screen, Screen::Closed) {
            self.screen = Screen::Closed;
            controls.push(InputControl::Release);
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let (cursor, len) = match &self.screen {
            Screen::Guilds { cursor, .. } => (*cursor, self.guilds.len()),
            Screen::Channels { cursor, guild, .. } => (*cursor, self.channel_rows(*guild).len()),
            Screen::Closed => return,
        };
        if len == 0 {
            return;
        }
        let next = (cursor as isize + delta).rem_euclid(len as isize) as usize;
        match &mut self.screen {
            Screen::Guilds { cursor, .. } => *cursor = next,
            Screen::Channels { cursor, .. } => *cursor = next,
            Screen::Closed => {}
        }
    }

    fn confirm(&mut self, cmds: &mut Vec<DiscordCommand>, controls: &mut Vec<InputControl>) {
        match self.screen.clone() {
            Screen::Guilds {
                cursor,
                loading: false,
            } => {
                if let Some(g) = self.guilds.get(cursor) {
                    let guild = g.id;
                    self.channels.clear();
                    self.screen = Screen::Channels {
                        guild,
                        cursor: 0,
                        loading: true,
                    };
                    cmds.push(DiscordCommand::ListVoiceChannels { guild });
                }
            }
            Screen::Channels {
                guild,
                cursor,
                loading: false,
            } => {
                let rows = self.channel_rows(guild);
                match rows.get(cursor) {
                    Some(ChannelRow::Leave) => {
                        self.leave(cmds);
                        self.close_menu(controls);
                    }
                    Some(ChannelRow::Channel(idx)) => {
                        let ch = self.channels[*idx].clone();
                        if self.connection.as_ref().map(|c| c.channel) == Some(ch.id) {
                            // toggle: confirming the connected channel leaves it
                            self.leave(cmds);
                        } else {
                            cmds.push(DiscordCommand::JoinVoice { channel: ch.id });
                            cmds.push(DiscordCommand::SubscribeVoice { channel: ch.id });
                            self.connection = Some(Connection {
                                channel: ch.id,
                                name: ch.name,
                                members: Vec::new(),
                            });
                        }
                        self.close_menu(controls);
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn leave(&mut self, cmds: &mut Vec<DiscordCommand>) {
        if let Some(c) = self.connection.take() {
            cmds.push(DiscordCommand::LeaveVoice);
            cmds.push(DiscordCommand::UnsubscribeVoice { channel: c.channel });
        }
    }

    fn back(&mut self, controls: &mut Vec<InputControl>) {
        match &self.screen {
            Screen::Channels { .. } => {
                self.screen = Screen::Guilds {
                    cursor: 0,
                    loading: self.guilds.is_empty(),
                };
            }
            Screen::Guilds { .. } => self.close_menu(controls),
            Screen::Closed => {}
        }
    }

    // ---- discord events ---------------------------------------------------

    pub fn on_discord(&mut self, ev: DiscordEvent) -> Step {
        match ev {
            DiscordEvent::Guilds(g) => {
                self.guilds = g;
                if let Screen::Guilds { loading, cursor } = &mut self.screen {
                    *loading = false;
                    *cursor = (*cursor).min(self.guilds.len().saturating_sub(1));
                }
            }
            DiscordEvent::VoiceChannels { guild, channels } => {
                if let Screen::Channels {
                    guild: g, loading, ..
                } = &mut self.screen
                {
                    if *g == guild {
                        *loading = false;
                    }
                }
                self.channels = channels;
            }
            DiscordEvent::JoinedVoice { channel } => {
                if self.connection.is_none() {
                    let name = self
                        .channels
                        .iter()
                        .find(|c| c.id == channel)
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    self.connection = Some(Connection {
                        channel,
                        name,
                        members: Vec::new(),
                    });
                } else if let Some(c) = &mut self.connection {
                    c.channel = channel;
                }
            }
            DiscordEvent::LeftVoice => self.connection = None,
            DiscordEvent::VoiceMembers { channel, members } => {
                if let Some(c) = &mut self.connection {
                    if c.channel == channel {
                        c.members = members;
                    }
                }
            }
            DiscordEvent::SpeakingChanged {
                channel,
                user,
                speaking,
            } => {
                if let Some(c) = &mut self.connection {
                    if c.channel == channel {
                        if let Some(m) = c.members.iter_mut().find(|m| m.user == user) {
                            m.speaking = speaking;
                        }
                    }
                }
            }
            DiscordEvent::Disconnected { reason } => {
                let _ = reason; // reserved for a recovery banner
                self.connection = None;
            }
            DiscordEvent::Connected { .. } => {}
            _ => {} // future events: ignored until handled
        }
        self.step(Vec::new(), Vec::new())
    }

    // ---- config -----------------------------------------------------------

    pub fn on_config(&mut self, cfg: &Config) -> Step {
        // Only adopt a new default anchor if not connected (don't yank a live HUD).
        if self.connection.is_none() {
            self.anchor = cfg.anchor;
        }
        self.step(Vec::new(), Vec::new())
    }

    // ---- scene assembly ---------------------------------------------------

    fn step(&self, cmds: Vec<DiscordCommand>, controls: Vec<InputControl>) -> Step {
        Step {
            cmds,
            controls,
            scene: self.scene(),
        }
    }

    fn scene(&self) -> Scene {
        Scene {
            menu: self.menu_view(),
            overlay: self.overlay(),
        }
    }

    fn menu_view(&self) -> Option<MenuView> {
        match &self.screen {
            Screen::Closed => None,
            Screen::Guilds { cursor, loading } => {
                if *loading {
                    return Some(loading_view("Servers"));
                }
                let rows = self
                    .guilds
                    .iter()
                    .map(|g| Row {
                        label: g.name.clone(),
                        icon: g.icon.clone(),
                        state: RowState::Normal,
                    })
                    .collect::<Vec<_>>();
                Some(MenuView {
                    title: "Servers".into(),
                    selected: clamp_sel(*cursor, &rows),
                    rows,
                })
            }
            Screen::Channels {
                cursor, loading, ..
            } => {
                if *loading {
                    return Some(loading_view("Voice Channels"));
                }
                let mut rows = Vec::new();
                if self.connection.is_some() {
                    rows.push(Row {
                        label: "⏏ Leave voice".into(),
                        icon: None,
                        state: RowState::Action,
                    });
                }
                for ch in &self.channels {
                    let active = self.connection.as_ref().map(|c| c.channel) == Some(ch.id);
                    let label = match ch.kind {
                        VoiceKind::Stage => format!("🎙 {}", ch.name),
                        _ => ch.name.clone(), // Guild + any future kind
                    };
                    rows.push(Row {
                        label,
                        icon: None,
                        state: if active {
                            RowState::Active
                        } else {
                            RowState::Normal
                        },
                    });
                }
                Some(MenuView {
                    title: "Voice Channels".into(),
                    selected: clamp_sel(*cursor, &rows),
                    rows,
                })
            }
        }
    }

    fn overlay(&self) -> Option<Overlay> {
        self.connection.as_ref().map(|c| Overlay {
            anchor: self.anchor,
            roster: Roster {
                channel_name: c.name.clone(),
                members: c.members.clone(),
            },
        })
    }

    /// Rows of the channels screen, with the synthetic Leave row when connected.
    fn channel_rows(&self, _guild: GuildId) -> Vec<ChannelRow> {
        let mut rows = Vec::new();
        if self.connection.is_some() {
            rows.push(ChannelRow::Leave);
        }
        for i in 0..self.channels.len() {
            rows.push(ChannelRow::Channel(i));
        }
        rows
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChannelRow {
    Leave,
    Channel(usize),
}

fn loading_view(title: &str) -> MenuView {
    MenuView {
        title: title.into(),
        rows: vec![Row {
            label: "Loading…".into(),
            icon: None,
            state: RowState::Loading,
        }],
        selected: 0,
    }
}

fn clamp_sel(cursor: usize, rows: &[Row]) -> usize {
    if rows.is_empty() {
        0
    } else {
        cursor.min(rows.len() - 1)
    }
}

#[cfg(test)]
mod tests;
