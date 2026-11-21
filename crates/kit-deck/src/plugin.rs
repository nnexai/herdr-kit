use crate::actions;
use crate::model::DeckSnapshot;
use crate::render::{
    render_agent_slot, render_coordinator, render_inbox, render_inbox_action, render_limit_bucket,
    render_limit_provider, render_overview, svg_to_data_uri,
};
use crate::sources::{SourceConfig, start_background};
use openaction::global_events::{GlobalEventHandler, set_global_event_handler};
use openaction::{Action, Instance, InstanceId, OpenActionResult, async_trait, register_action};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, RwLock, watch};

pub const UUID_OVERVIEW: &str = "dev.herdr.kit.overview";
pub const UUID_INBOX: &str = "dev.herdr.kit.inbox";
pub const UUID_INBOX_ACTION: &str = "dev.herdr.kit.inbox-action";
pub const UUID_AGENT: &str = "dev.herdr.kit.agent";
pub const UUID_COORDINATOR: &str = "dev.herdr.kit.coordinator";
pub const UUID_LIMITS: &str = "dev.herdr.kit.limits";
pub const UUID_LIMIT_BUCKET: &str = "dev.herdr.kit.limit-bucket";

const ALL_UUIDS: [&str; 7] = [
    UUID_OVERVIEW,
    UUID_INBOX,
    UUID_INBOX_ACTION,
    UUID_AGENT,
    UUID_COORDINATOR,
    UUID_LIMITS,
    UUID_LIMIT_BUCKET,
];

const HOLD_THRESHOLD: Duration = Duration::from_millis(650);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EmptySettings {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSettings {
    #[serde(default = "default_slot")]
    pub slot: usize,
    #[serde(default = "default_filter")]
    pub filter: String,
    #[serde(default)]
    pub pin_space: Option<String>,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            slot: default_slot(),
            filter: default_filter(),
            pin_space: None,
        }
    }
}

fn default_slot() -> usize {
    1
}
fn default_filter() -> String {
    "all".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LimitsSettings {
    #[serde(default = "default_worst")]
    pub provider: String,
}

impl Default for LimitsSettings {
    fn default() -> Self {
        Self {
            provider: default_worst(),
        }
    }
}

fn default_worst() -> String {
    "worst".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BucketSettings {
    #[serde(default = "default_selected")]
    pub provider: String,
    #[serde(default)]
    pub bucket_index: usize,
}

impl Default for BucketSettings {
    fn default() -> Self {
        Self {
            provider: default_selected(),
            bucket_index: 0,
        }
    }
}

fn default_selected() -> String {
    "selected".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxActionSettings {
    #[serde(default = "default_next")]
    pub mode: String,
}

impl Default for InboxActionSettings {
    fn default() -> Self {
        Self {
            mode: default_next(),
        }
    }
}

fn default_next() -> String {
    "next".into()
}

#[derive(Clone, Debug)]
enum StoredSettings {
    Empty,
    Agent(AgentSettings),
    Limits(LimitsSettings),
    Bucket(BucketSettings),
    InboxAction(InboxActionSettings),
}

struct PluginCore {
    cfg: SourceConfig,
    snapshot_rx: watch::Receiver<Arc<DeckSnapshot>>,
    refresh_notify: Arc<Notify>,
    ui_notify: Arc<Notify>,
    settings: RwLock<HashMap<InstanceId, StoredSettings>>,
    pressed_at: RwLock<HashMap<InstanceId, Instant>>,
    last_svg: RwLock<HashMap<InstanceId, String>>,
    inbox_index: AtomicUsize,
    selected_provider: RwLock<String>,
    hold_hint_until: RwLock<Option<Instant>>,
}

static GLOBAL_HANDLER: OnceLock<GlobalReadyHook> = OnceLock::new();

struct GlobalReadyHook {
    core: Arc<PluginCore>,
}

#[async_trait]
impl GlobalEventHandler for GlobalReadyHook {
    async fn plugin_ready(&self) -> OpenActionResult<()> {
        let core = self.core.clone();
        tokio::spawn(async move {
            core.render_loop().await;
        });
        Ok(())
    }
}

impl PluginCore {
    async fn on_appear(&self, instance: &Instance, stored: StoredSettings) {
        self.settings
            .write()
            .await
            .insert(instance.instance_id.clone(), stored);
        self.render_instance(instance).await;
    }

    async fn on_disappear(&self, instance: &Instance) {
        self.settings.write().await.remove(&instance.instance_id);
        self.pressed_at.write().await.remove(&instance.instance_id);
        self.last_svg.write().await.remove(&instance.instance_id);
    }

    async fn on_settings(&self, instance: &Instance, stored: StoredSettings) {
        self.settings
            .write()
            .await
            .insert(instance.instance_id.clone(), stored);
        self.render_instance(instance).await;
    }

    async fn on_key_down(&self, instance: &Instance) {
        self.pressed_at
            .write()
            .await
            .insert(instance.instance_id.clone(), Instant::now());
    }

    async fn take_hold(&self, instance: &Instance) -> bool {
        let down = self.pressed_at.write().await.remove(&instance.instance_id);
        down.is_some_and(|t| t.elapsed() >= HOLD_THRESHOLD)
    }

    async fn render_svg_for(&self, uuid: &str, instance_id: &str, snap: &DeckSnapshot) -> String {
        let stored = self.settings.read().await.get(instance_id).cloned();
        let selected_provider = self.selected_provider.read().await.clone();
        let inbox_idx = self.inbox_index.load(Ordering::Relaxed);
        let hold_hint = self
            .hold_hint_until
            .read()
            .await
            .is_some_and(|until| Instant::now() < until);
        match uuid {
            UUID_OVERVIEW => render_overview(snap),
            UUID_INBOX => render_inbox(snap, inbox_idx),
            UUID_INBOX_ACTION => {
                let mode = match stored {
                    Some(StoredSettings::InboxAction(s)) => s.mode,
                    _ => "next".into(),
                };
                render_inbox_action(snap, inbox_idx, &mode, hold_hint)
            }
            UUID_AGENT => {
                let s = match stored {
                    Some(StoredSettings::Agent(s)) => s,
                    _ => AgentSettings::default(),
                };
                render_agent_slot(snap, s.slot, &s.filter, s.pin_space.as_deref())
            }
            UUID_COORDINATOR => render_coordinator(snap),
            UUID_LIMITS => {
                let provider = match stored {
                    Some(StoredSettings::Limits(s)) => s.provider,
                    _ => "worst".into(),
                };
                render_limit_provider(snap, &provider, &selected_provider)
            }
            UUID_LIMIT_BUCKET => {
                let s = match stored {
                    Some(StoredSettings::Bucket(s)) => s,
                    _ => BucketSettings::default(),
                };
                render_limit_bucket(snap, &s.provider, &selected_provider, s.bucket_index)
            }
            _ => render_overview(snap),
        }
    }

    async fn render_instance(&self, instance: &Instance) {
        let snap = self.snapshot_rx.borrow().clone();
        let svg = self
            .render_svg_for(&instance.action_uuid, &instance.instance_id, &snap)
            .await;
        let unchanged = self
            .last_svg
            .read()
            .await
            .get(&instance.instance_id)
            .is_some_and(|prev| prev == &svg);
        if unchanged {
            return;
        }
        let uri = svg_to_data_uri(&svg);
        if instance.set_image(Some(uri), None).await.is_ok() {
            let _ = instance.set_title(Some(""), None).await;
            self.last_svg
                .write()
                .await
                .insert(instance.instance_id.clone(), svg);
        }
    }

    async fn render_all_visible(&self) {
        for uuid in ALL_UUIDS {
            for instance in openaction::visible_instances(uuid).await {
                self.render_instance(&instance).await;
            }
        }
    }

    async fn render_loop(self: Arc<Self>) {
        let mut rx = self.snapshot_rx.clone();
        loop {
            self.render_all_visible().await;
            tokio::select! {
                res = rx.changed() => {
                    if res.is_err() {
                        break;
                    }
                }
                _ = self.ui_notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            }
        }
    }

    async fn flash_hold_hint(&self) {
        *self.hold_hint_until.write().await = Some(Instant::now() + Duration::from_secs(2));
        self.ui_notify.notify_one();
        let ui = self.ui_notify.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            ui.notify_one();
        });
    }

    fn run_bg_action(
        &self,
        instance: &Instance,
        fut: impl std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        let instance_id = instance.instance_id.clone();
        let refresh = self.refresh_notify.clone();
        tokio::spawn(async move {
            let result = fut.await;
            if let Some(inst) = openaction::get_instance(instance_id).await {
                let _ = match result {
                    Ok(()) => inst.show_ok().await,
                    Err(_) => inst.show_alert().await,
                };
            }
            refresh.notify_one();
        });
    }
    async fn handle_key_up(&self, instance: &Instance, stored: StoredSettings) {
        let hold = self.take_hold(instance).await;
        let snap = self.snapshot_rx.borrow().clone();
        match (instance.action_uuid.as_str(), stored) {
            (UUID_OVERVIEW, _) => {
                if hold {
                    self.refresh_notify.notify_one();
                    let _ = instance.show_ok().await;
                    return;
                }
                let target = snap
                    .inbox
                    .first()
                    .and_then(|i| i.terminal.clone())
                    .or_else(|| snap.agents.first().map(|a| a.address.clone()));
                if let Some(term) = target {
                    let cfg = self.cfg.clone();
                    self.run_bg_action(instance, async move {
                        actions::focus_terminal(&cfg, &term).await
                    });
                }
            }
            (UUID_INBOX, _) => {
                if hold {
                    if !snap.inbox.is_empty() {
                        self.inbox_index.fetch_add(1, Ordering::Relaxed);
                        self.ui_notify.notify_one();
                    }
                    return;
                }
                let idx = self.inbox_index.load(Ordering::Relaxed);
                if let Some(term) = snap
                    .inbox_item(idx)
                    .and_then(|(item, _)| item.terminal.clone())
                {
                    let cfg = self.cfg.clone();
                    self.run_bg_action(instance, async move {
                        actions::focus_terminal(&cfg, &term).await
                    });
                }
            }
            (UUID_INBOX_ACTION, StoredSettings::InboxAction(settings)) => {
                match settings.mode.as_str() {
                    "prev" => {
                        if !snap.inbox.is_empty() {
                            let cur = self.inbox_index.load(Ordering::Relaxed);
                            let prev = (cur + snap.inbox.len() - 1) % snap.inbox.len();
                            self.inbox_index.store(prev, Ordering::Relaxed);
                            self.ui_notify.notify_one();
                        }
                    }
                    "focus" => {
                        let idx = self.inbox_index.load(Ordering::Relaxed);
                        if let Some(term) = snap
                            .inbox_item(idx)
                            .and_then(|(item, _)| item.terminal.clone())
                        {
                            let cfg = self.cfg.clone();
                            self.run_bg_action(instance, async move {
                                actions::focus_terminal(&cfg, &term).await
                            });
                        }
                    }
                    "reply_1" | "reply_2" => {
                        if !hold {
                            self.flash_hold_hint().await;
                            return;
                        }
                        let opt_idx = if settings.mode == "reply_1" { 0 } else { 1 };
                        let idx = self.inbox_index.load(Ordering::Relaxed);
                        if let Some((item, _)) = snap.inbox_item(idx) {
                            if let (Some(qid), Some(text)) =
                                (item.qid.clone(), item.options.get(opt_idx).cloned())
                            {
                                let cfg = self.cfg.clone();
                                self.run_bg_action(instance, async move {
                                    actions::answer_question(&cfg, &qid, &text).await
                                });
                            }
                        }
                    }
                    _ => {
                        if !snap.inbox.is_empty() {
                            self.inbox_index.fetch_add(1, Ordering::Relaxed);
                            self.ui_notify.notify_one();
                        }
                    }
                }
            }
            (UUID_AGENT, StoredSettings::Agent(settings)) => {
                let Some(agent) = snap.agent_for_slot(
                    settings.slot,
                    &settings.filter,
                    settings.pin_space.as_deref(),
                ) else {
                    self.refresh_notify.notify_one();
                    return;
                };
                let term = agent.address.clone();
                let cfg = self.cfg.clone();
                if hold && agent.status == crate::model::AgentStateKind::Working {
                    self.run_bg_action(instance, async move {
                        actions::interrupt_terminal(&cfg, &term).await
                    });
                } else if hold {
                    self.refresh_notify.notify_one();
                    let _ = instance.show_ok().await;
                } else {
                    self.run_bg_action(instance, async move {
                        actions::focus_terminal(&cfg, &term).await
                    });
                }
            }
            (UUID_COORDINATOR, _) => {
                let cfg = self.cfg.clone();
                if hold {
                    self.run_bg_action(
                        instance,
                        async move { actions::start_coordinator(&cfg).await },
                    );
                } else if let Some(term) =
                    snap.coordinator.as_ref().and_then(|c| c.terminal.clone())
                {
                    self.run_bg_action(instance, async move {
                        actions::focus_terminal(&cfg, &term).await
                    });
                } else {
                    self.refresh_notify.notify_one();
                }
            }
            (UUID_LIMITS, StoredSettings::Limits(settings)) => {
                if hold {
                    self.refresh_notify.notify_one();
                    let _ = instance.show_ok().await;
                    return;
                }
                let mut selected = self.selected_provider.write().await;
                if settings.provider == "worst" || settings.provider.is_empty() {
                    let order = ["codex", "claude", "copilot", "antigravity"];
                    let cur_idx = order
                        .iter()
                        .position(|p| *p == selected.as_str())
                        .unwrap_or(0);
                    *selected = order[(cur_idx + 1) % order.len()].to_owned();
                } else {
                    *selected = settings.provider;
                }
                drop(selected);
                self.ui_notify.notify_one();
            }
            (UUID_LIMIT_BUCKET, _) => {
                self.refresh_notify.notify_one();
                let _ = instance.show_ok().await;
            }
            _ => {}
        }
    }
}

macro_rules! deck_action {
    ($name:ident, $uuid:expr, $settings_ty:ty, $wrap:expr) => {
        struct $name {
            core: Arc<PluginCore>,
        }

        #[async_trait]
        impl Action for $name {
            const UUID: &'static str = $uuid;
            type Settings = $settings_ty;

            async fn will_appear(
                &self,
                instance: &Instance,
                settings: &Self::Settings,
            ) -> OpenActionResult<()> {
                self.core.on_appear(instance, ($wrap)(settings)).await;
                Ok(())
            }

            async fn will_disappear(
                &self,
                instance: &Instance,
                _settings: &Self::Settings,
            ) -> OpenActionResult<()> {
                self.core.on_disappear(instance).await;
                Ok(())
            }

            async fn did_receive_settings(
                &self,
                instance: &Instance,
                settings: &Self::Settings,
            ) -> OpenActionResult<()> {
                self.core.on_settings(instance, ($wrap)(settings)).await;
                Ok(())
            }

            async fn key_down(
                &self,
                instance: &Instance,
                _settings: &Self::Settings,
            ) -> OpenActionResult<()> {
                self.core.on_key_down(instance).await;
                Ok(())
            }

            async fn key_up(
                &self,
                instance: &Instance,
                settings: &Self::Settings,
            ) -> OpenActionResult<()> {
                self.core.handle_key_up(instance, ($wrap)(settings)).await;
                Ok(())
            }
        }
    };
}

deck_action!(OverviewAction, UUID_OVERVIEW, EmptySettings, |_| {
    StoredSettings::Empty
});
deck_action!(InboxAction, UUID_INBOX, EmptySettings, |_| {
    StoredSettings::Empty
});
deck_action!(
    InboxControlAction,
    UUID_INBOX_ACTION,
    InboxActionSettings,
    |s: &InboxActionSettings| StoredSettings::InboxAction(s.clone())
);
deck_action!(
    AgentSlotAction,
    UUID_AGENT,
    AgentSettings,
    |s: &AgentSettings| StoredSettings::Agent(s.clone())
);
deck_action!(CoordinatorAction, UUID_COORDINATOR, EmptySettings, |_| {
    StoredSettings::Empty
});
deck_action!(
    LimitsAction,
    UUID_LIMITS,
    LimitsSettings,
    |s: &LimitsSettings| StoredSettings::Limits(s.clone())
);
deck_action!(
    LimitBucketAction,
    UUID_LIMIT_BUCKET,
    BucketSettings,
    |s: &BucketSettings| StoredSettings::Bucket(s.clone())
);

pub async fn run(cfg: SourceConfig, args: Vec<String>) -> anyhow::Result<()> {
    let (snapshot_rx, refresh_notify) = start_background(cfg.clone());
    let core = Arc::new(PluginCore {
        cfg,
        snapshot_rx,
        refresh_notify,
        ui_notify: Arc::new(Notify::new()),
        settings: RwLock::new(HashMap::new()),
        pressed_at: RwLock::new(HashMap::new()),
        last_svg: RwLock::new(HashMap::new()),
        inbox_index: AtomicUsize::new(0),
        selected_provider: RwLock::new("antigravity".into()),
        hold_hint_until: RwLock::new(None),
    });

    if let Some(hook) = GLOBAL_HANDLER
        .set(GlobalReadyHook { core: core.clone() })
        .ok()
        .map(|()| GLOBAL_HANDLER.get().unwrap())
    {
        set_global_event_handler(hook);
    }

    register_action(OverviewAction { core: core.clone() }).await;
    register_action(InboxAction { core: core.clone() }).await;
    register_action(InboxControlAction { core: core.clone() }).await;
    register_action(AgentSlotAction { core: core.clone() }).await;
    register_action(CoordinatorAction { core: core.clone() }).await;
    register_action(LimitsAction { core: core.clone() }).await;
    register_action(LimitBucketAction { core }).await;

    openaction::run(args).await?;
    Ok(())
}
