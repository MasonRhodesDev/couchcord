//! The composition-root reactor: the one place that drives the pure `MenuEngine`
//! against the live boundary traits (`RpcClient`, `InputSource`, `OverlayRenderer`,
//! `ConfigSource`). It owns no domain knowledge — it only *wires* domains.
//!
//! `Dispatcher` is generic over the four boundaries so it can be exercised with
//! mocks (see tests) exactly as it will be with the real impls. The full
//! `tokio::select!` event loop that feeds it from the input + voice streams is
//! `run()`; the per-event dispatch logic lives in `Dispatcher` and is unit-tested.

use cc_core::{
    Anchor, ConfigSource, DiscordCommand, DiscordEvent, InputIntent, InputSource, NavGuard,
    OverlayRenderer, RpcClient, VoiceEvent,
};
use cc_menu::{MenuEngine, Step};
use std::collections::VecDeque;
use tokio::sync::mpsc;

pub mod run;

/// Drives one `MenuEngine` against the boundaries. Generic for testability.
pub struct Dispatcher<R, I, Rn, C> {
    engine: MenuEngine,
    rpc: R,
    input: I,
    render: Rn,
    config: C,
    nav: Option<NavGuard>,
    last_anchor: Anchor,
    /// When set, voice subscriptions are drained into this sink as
    /// `DiscordEvent`s for the run loop to feed back in.
    voice_sink: Option<mpsc::Sender<DiscordEvent>>,
}

impl<R, I, Rn, C> Dispatcher<R, I, Rn, C>
where
    R: RpcClient,
    I: InputSource,
    Rn: OverlayRenderer,
    C: ConfigSource,
{
    pub fn new(engine: MenuEngine, rpc: R, input: I, render: Rn, config: C) -> Self {
        let last_anchor = engine.anchor();
        Dispatcher {
            engine,
            rpc,
            input,
            render,
            config,
            nav: None,
            last_anchor,
            voice_sink: None,
        }
    }

    /// Route drained voice-subscription events into `sink` (the live run loop).
    pub fn set_voice_sink(&mut self, sink: mpsc::Sender<DiscordEvent>) {
        self.voice_sink = Some(sink);
    }

    /// Feed a controller intent and fully apply the consequences.
    pub async fn on_input(&mut self, intent: InputIntent) {
        let step = self.engine.on_input(intent);
        self.drive(step).await;
    }

    /// Feed a Discord event and fully apply the consequences.
    pub async fn on_discord(&mut self, ev: DiscordEvent) {
        let step = self.engine.on_discord(ev);
        self.drive(step).await;
    }

    /// Apply a `Step`, then iteratively apply any follow-up events produced by
    /// its commands (e.g. `ListGuilds` → fetch → `Guilds(..)` → new `Step`).
    /// Iterative (a queue), not recursive, so it stays a plain async fn.
    async fn drive(&mut self, first: Step) {
        let mut queue: VecDeque<Step> = VecDeque::new();
        queue.push_back(first);
        while let Some(step) = queue.pop_front() {
            // 1. input capture transitions
            for ctl in &step.controls {
                match ctl {
                    cc_core::InputControl::Grab => {
                        self.nav = self.input.grab().ok();
                    }
                    cc_core::InputControl::Release => {
                        self.nav = None; // dropping the guard ungrabs
                    }
                    _ => {}
                }
            }
            // 2. paint the frame (loading screens show before data arrives)
            let _ = self.render.draw(&step.scene).await;
            // 3. persist an anchor change (ConfigSource, never a message)
            let a = self.engine.anchor();
            if a != self.last_anchor {
                self.config.store_anchor(a);
                self.last_anchor = a;
            }
            // 4. run commands; queue any follow-up event
            for cmd in step.cmds {
                if let Some(ev) = self.run_cmd(cmd).await {
                    queue.push_back(self.engine.on_discord(ev));
                }
            }
        }
    }

    /// Execute one Discord command; return the domain event it yields, if any.
    async fn run_cmd(&mut self, cmd: DiscordCommand) -> Option<DiscordEvent> {
        match cmd {
            DiscordCommand::ListGuilds => match self.rpc.guilds().await {
                Ok(g) => Some(DiscordEvent::Guilds(g)),
                Err(_) => Some(DiscordEvent::Disconnected {
                    reason: cc_core::DisconnectReason::SocketClosed,
                }),
            },
            DiscordCommand::ListVoiceChannels { guild } => {
                match self.rpc.voice_channels(guild).await {
                    Ok(channels) => Some(DiscordEvent::VoiceChannels { guild, channels }),
                    Err(_) => None,
                }
            }
            DiscordCommand::JoinVoice { channel } => {
                let _ = self.rpc.select_voice(Some(channel)).await;
                Some(DiscordEvent::JoinedVoice { channel })
            }
            DiscordCommand::LeaveVoice => {
                let _ = self.rpc.select_voice(None).await;
                Some(DiscordEvent::LeftVoice)
            }
            // Subscriptions: drain the per-channel stream into the voice sink
            // (when wired by the run loop). No immediate follow-up event.
            DiscordCommand::SubscribeVoice { channel } => {
                if let Some(sink) = self.voice_sink.clone() {
                    let stream = self.rpc.subscribe_voice(channel);
                    tokio::spawn(async move {
                        use futures_util::StreamExt;
                        let mut stream = stream;
                        while let Some(ev) = stream.next().await {
                            let de = match ev {
                                VoiceEvent::Members { channel, members } => {
                                    DiscordEvent::VoiceMembers { channel, members }
                                }
                                VoiceEvent::SpeakingChanged { channel, user, speaking } => {
                                    DiscordEvent::SpeakingChanged { channel, user, speaking }
                                }
                                _ => continue,
                            };
                            if sink.send(de).await.is_err() {
                                break;
                            }
                        }
                    });
                }
                None
            }
            DiscordCommand::UnsubscribeVoice { .. } | DiscordCommand::Connect => None,
            _ => None,
        }
    }

    /// Test/inspection: the engine's current anchor.
    pub fn anchor(&self) -> Anchor {
        self.engine.anchor()
    }
}

#[cfg(test)]
mod tests;
