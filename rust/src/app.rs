use crate::bridge;
use crate::catalog::{self, SUPPORTED_GENRES};
use crate::companion::{self, CompanionServerHandle, CompanionSubmission};
use crate::images::{ImageCache, ImgKind};
use crate::models::*;
use crate::store;
use crate::sync::{self, PollTokenResult};

use chrono::Utc;
use egui::{
    Align, Align2, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily,
    FontId, Key, Mesh, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, Vec2,
};
use qrcode::QrCode;
use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavTab {
    Deck,
    Collection,
    SyncHub,
    Themes,
}

impl NavTab {
    const ALL: [NavTab; 4] = [
        NavTab::Deck,
        NavTab::Collection,
        NavTab::SyncHub,
        NavTab::Themes,
    ];
    fn label(self) -> &'static str {
        match self {
            NavTab::Deck => "Deck",
            NavTab::Collection => "Collection",
            NavTab::SyncHub => "Sync Hub",
            NavTab::Themes => "Themes",
        }
    }
}

#[derive(Debug, Clone)]
enum ModalState {
    None,
    Filters,
    Details(MediaItem),
    DeviceAuth(DeviceAuthSession),
    BuyMeACoffee,
}

enum BgEvent {
    DeckLoaded {
        reset: bool,
        page: usize,
        items: Vec<MediaItem>,
    },
    ItemEnriched(MediaItem),
    TrailerResolved {
        item: MediaItem,
        yt_id: Option<String>,
    },
    CompanionCreds(CompanionSubmission),
    DeviceCodeReady(Result<DeviceAuthSession, String>),
    AuthPollOutcome {
        provider: SyncProvider,
        outcome: PollTokenResult,
        username: Option<String>,
    },
    RemoteSyncDone {
        trakt_entries: Option<Vec<TrackedEntry>>,
        simkl_entries: Option<Vec<TrackedEntry>>,
        flushed_op_ids: Vec<String>,
        silent: bool,
    },
}

struct FlyOutAnim {
    item: MediaItem,
    action: SwipeAction,
    started_at: Instant,
    duration: f32,
}

struct RewindAnim {
    started_at: Instant,
    duration: f32,
}

struct ToastBanner {
    text: String,
    accent: Option<SwipeAction>,
    shown_at: Instant,
}

pub struct SwayApp {
    state: PersistedState,
    images: ImageCache,
    logo_tex: Option<TextureHandle>,
    qr_cache: Option<(String, TextureHandle)>,
    companion: CompanionServerHandle,

    tx: Sender<BgEvent>,
    rx: Receiver<BgEvent>,

    active_tab: NavTab,
    top_rail_focused: bool,
    top_rail_idx: usize, // 0..4 tabs, 4=Undo, 5=Filters

    // Deck state
    media_type: MediaType,
    category: FeedCategory,
    genre_idx: usize,
    search_query: String,
    deck: Vec<MediaItem>,
    deck_page: usize,
    loading_deck: bool,
    enriched_ids: HashSet<String>,
    undo_stack: Vec<SwipeHistoryStep>,
    deck_btn_idx: usize,
    fly_anim: Option<FlyOutAnim>,
    rewind_anim: Option<RewindAnim>,

    // Filter modal focus
    filter_row: usize, // 0=MediaType, 1=Category, 2=Genre, 3=Search, 4=Apply/Close

    // Detail modal focus
    detail_btn_idx: usize, // 0=Watch 4K Trailer, 1=Watchlist, 2=Watched, 3=Pass, 4=Rate -/+, 5=Close
    resolving_trailer: bool,

    // Collection state
    collection_subtab: usize, // 0=Watchlist, 1=Watched
    collection_focus: usize,
    collection_act_idx: usize, // 0=Details/Trailer, 1=Move, 2=Remove

    // Sync Hub state
    sync_focus: usize, // 0=Trakt Connect/Disconnect, 1=Trakt Sync, 2=Simkl Connect/Disconnect, 3=Simkl Sync
    sync_in_progress: bool,
    last_auth_poll: Instant,
    auth_poll_inflight: bool,

    // Themes & Settings state
    settings_focus: usize, // 0..6 theme presets, 6..16 settings rows

    modal: ModalState,
    toast: Option<ToastBanner>,
    start_time: Instant,
    last_back_at: Instant,
}

impl SwayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        bridge::set_egui_ctx(cc.egui_ctx.clone());
        Self::install_fonts(&cc.egui_ctx);

        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::TRANSPARENT;
        style.visuals.window_fill = Color32::from_rgb(10, 14, 18);
        cc.egui_ctx.set_style(style);

        // Set crisp 2.0 DPI scale on 3840x2160 4K TVs so 1920x1080 logical points look razor-sharp
        cc.egui_ctx.set_pixels_per_point(2.0);

        let state = store::load_state();
        let (tx, rx) = mpsc::channel();

        let tx_comp = tx.clone();
        let ctx_comp = cc.egui_ctx.clone();
        let companion = companion::start_companion_server(state.credentials.clone(), move |sub| {
            let _ = tx_comp.send(BgEvent::CompanionCreds(sub));
            ctx_comp.request_repaint();
        });

        let logo_tex = image::load_from_memory(include_bytes!("../assets/sway_logo_mark.png"))
            .ok()
            .map(|img| {
                let rgba = img.to_rgba8();
                let ci = ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    rgba.as_raw(),
                );
                cc.egui_ctx
                    .load_texture("sway_brand_logo", ci, TextureOptions::LINEAR)
            });

        let now = Instant::now();
        let mut app = Self {
            state,
            images: ImageCache::new(),
            logo_tex,
            qr_cache: None,
            companion,
            tx,
            rx,
            active_tab: NavTab::Deck,
            top_rail_focused: false,
            top_rail_idx: 0,
            media_type: MediaType::Show,
            category: FeedCategory::Trending,
            genre_idx: 0,
            search_query: String::new(),
            deck: catalog::fallback_seed_deck(MediaType::Show),
            deck_page: 0,
            loading_deck: false,
            enriched_ids: HashSet::new(),
            undo_stack: Vec::new(),
            deck_btn_idx: 1,
            fly_anim: None,
            rewind_anim: None,
            filter_row: 0,
            detail_btn_idx: 0,
            resolving_trailer: false,
            collection_subtab: 0,
            collection_focus: 0,
            collection_act_idx: 0,
            sync_focus: 0,
            sync_in_progress: false,
            last_auth_poll: now,
            auth_poll_inflight: false,
            settings_focus: 0,
            modal: ModalState::None,
            toast: None,
            start_time: now,
            last_back_at: now - Duration::from_secs(2),
        };

        app.filter_deck_in_place();
        app.spawn_fetch_deck(cc.egui_ctx.clone(), true, 0);
        if !app.state.credentials.trakt_access_token.is_empty()
            || !app.state.credentials.simkl_access_token.is_empty()
        {
            app.spawn_sync_now(cc.egui_ctx.clone(), true);
        }
        app
    }

    fn install_fonts(ctx: &egui::Context) {
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            "jakarta".into(),
            Arc::new(FontData::from_static(include_bytes!(
                "../assets/fonts/plus_jakarta_sans_var.ttf"
            ))),
        );
        fonts.font_data.insert(
            "outfit".into(),
            Arc::new(FontData::from_static(include_bytes!(
                "../assets/fonts/outfit_var.ttf"
            ))),
        );
        fonts.font_data.insert(
            "fraunces".into(),
            Arc::new(FontData::from_static(include_bytes!(
                "../assets/fonts/fraunces_var.ttf"
            ))),
        );
        fonts.font_data.insert(
            "syne".into(),
            Arc::new(FontData::from_static(include_bytes!(
                "../assets/fonts/syne_var.ttf"
            ))),
        );
        fonts.font_data.insert(
            "space_grotesk".into(),
            Arc::new(FontData::from_static(include_bytes!(
                "../assets/fonts/space_grotesk_var.ttf"
            ))),
        );

        if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
            prop.insert(0, "jakarta".into());
        }
        for key in ["outfit", "fraunces", "syne", "space_grotesk"] {
            fonts.families.insert(
                FontFamily::Name(key.into()),
                vec![key.into(), "jakarta".into()],
            );
        }
        ctx.set_fonts(fonts);
    }

    fn display_font(&self, size: f32) -> FontId {
        FontId::new(
            size,
            FontFamily::Name(self.state.settings.font_pairing.family_key().into()),
        )
    }

    fn show_toast(&mut self, text: impl Into<String>, accent: Option<SwipeAction>) {
        self.toast = Some(ToastBanner {
            text: text.into(),
            accent,
            shown_at: Instant::now(),
        });
    }

    fn persist(&self) {
        store::save_state(&self.state);
    }

    fn filter_deck_in_place(&mut self) {
        if self.state.settings.hide_already_tracked {
            self.deck
                .retain(|item| !self.state.tracked.contains_key(&item.id));
        }
    }

    fn spawn_fetch_deck(&mut self, ctx: egui::Context, reset: bool, page: usize) {
        if self.loading_deck {
            return;
        }
        self.loading_deck = true;
        let tx = self.tx.clone();
        let mt = self.media_type;
        let cat = self.category;
        let genre = SUPPORTED_GENRES[self.genre_idx].to_string();
        let query = self.search_query.clone();
        let trakt_cid = self.state.credentials.trakt_client_id.clone();
        let trakt_tok = self.state.credentials.trakt_access_token.clone();

        std::thread::spawn(move || {
            let mut items = if !query.trim().is_empty() {
                catalog::search_catalog(&query, mt)
            } else {
                let mut list = Vec::new();
                if page == 0 && !trakt_cid.trim().is_empty() && genre == "All" {
                    list.extend(sync::trakt_fetch_discovery(&trakt_cid, &trakt_tok, cat, mt));
                }
                let cine = catalog::fetch_catalog_page(mt, cat, &genre, page);
                let mut seen: HashSet<String> = list.iter().map(|i| i.id.clone()).collect();
                for it in cine {
                    if seen.insert(it.id.clone()) {
                        list.push(it);
                    }
                }
                list
            };
            if items.is_empty() && page == 0 && query.trim().is_empty() {
                items = catalog::fallback_seed_deck(mt);
            }
            let _ = tx.send(BgEvent::DeckLoaded { reset, page, items });
            ctx.request_repaint();
        });
    }

    fn maybe_enrich_top_card(&mut self, ctx: &egui::Context) {
        let Some(top) = self.deck.first() else { return };
        if !top.cast.is_empty() && top.trailer_youtube_id.is_some() {
            return;
        }
        if !self.enriched_ids.insert(top.id.clone()) {
            return;
        }
        let item_c = top.clone();
        let tx = self.tx.clone();
        let ctx_c = ctx.clone();
        std::thread::spawn(move || {
            if let Some(enriched) = catalog::enrich_item_details(&item_c) {
                let _ = tx.send(BgEvent::ItemEnriched(enriched));
                ctx_c.request_repaint();
            }
        });
    }

    fn prefetch_upcoming_cards(&mut self, ctx: &egui::Context) {
        let blur_kind = if self.state.settings.backdrop_blur_enabled {
            ImgKind::BackdropBlurred
        } else {
            ImgKind::BackdropCrisp
        };
        for item in self.deck.iter().take(5) {
            self.images.prefetch(&item.poster_url, ImgKind::Poster, ctx);
            self.images.prefetch(&item.backdrop_url, blur_kind, ctx);
        }
    }

    fn apply_swipe(&mut self, ctx: &egui::Context, item: MediaItem, action: SwipeAction) {
        let prev = self.state.tracked.get(&item.id).cloned();
        self.undo_stack.push(SwipeHistoryStep {
            item: item.clone(),
            previous_entry: prev,
        });
        if self.undo_stack.len() > 30 {
            self.undo_stack.remove(0);
        }

        self.deck.retain(|c| c.id != item.id);
        self.fly_anim = Some(FlyOutAnim {
            item: item.clone(),
            action,
            started_at: Instant::now(),
            duration: self.state.settings.swipe_speed.duration_sec(),
        });

        let entry = TrackedEntry {
            item: item.clone(),
            action,
            updated_at_ms: Utc::now().timestamp_millis(),
            user_rating: self
                .state
                .tracked
                .get(&item.id)
                .and_then(|e| e.user_rating),
            synced_trakt: false,
            synced_simkl: false,
        };
        self.state.tracked.insert(item.id.clone(), entry);

        // Enqueue cloud sync ops for Watchlist / Watched
        let op_type_opt = match action {
            SwipeAction::Watched => Some(SyncOpType::AddWatched),
            SwipeAction::ToWatch => Some(SyncOpType::AddWatchlist),
            SwipeAction::Skipped => None,
        };
        if let Some(op_type) = op_type_opt {
            self.enqueue_sync_op(&item, op_type, None, ctx);
        }
        self.persist();

        let msg = match action {
            SwipeAction::ToWatch => format!("Added \"{}\" to Watchlist", item.title),
            SwipeAction::Watched => format!("Marked \"{}\" as Watched", item.title),
            SwipeAction::Skipped => format!("Passed on \"{}\"", item.title),
        };
        self.show_toast(msg, Some(action));

        if self.deck.len() <= 6 && !self.loading_deck && self.search_query.trim().is_empty() {
            let next_page = self.deck_page + 1;
            self.spawn_fetch_deck(ctx.clone(), false, next_page);
        }
    }

    fn undo_last_swipe(&mut self) {
        let Some(step) = self.undo_stack.pop() else {
            self.show_toast("Nothing to undo", None);
            return;
        };
        match step.previous_entry {
            Some(prev) => {
                self.state.tracked.insert(step.item.id.clone(), prev);
            }
            None => {
                self.state.tracked.remove(&step.item.id);
            }
        }
        self.deck.retain(|c| c.id != step.item.id);
        self.deck.insert(0, step.item.clone());
        self.rewind_anim = Some(RewindAnim {
            started_at: Instant::now(),
            duration: self.state.settings.swipe_speed.duration_sec(),
        });
        self.persist();
        self.show_toast(format!("Rewound \"{}\"", step.item.title), None);
    }

    fn enqueue_sync_op(
        &mut self,
        item: &MediaItem,
        op_type: SyncOpType,
        rating: Option<i32>,
        ctx: &egui::Context,
    ) {
        let mode = self.state.settings.sync_target_mode;
        let trakt_ok = !self.state.credentials.trakt_access_token.is_empty()
            && matches!(mode, SyncTargetMode::BothConnected | SyncTargetMode::TraktOnly);
        let simkl_ok = !self.state.credentials.simkl_access_token.is_empty()
            && matches!(mode, SyncTargetMode::BothConnected | SyncTargetMode::SimklOnly);

        let ts = Utc::now().timestamp_millis();
        if trakt_ok {
            self.state.pending_ops.push(PendingSyncOp {
                op_id: format!("trakt-{}-{ts}", item.id),
                provider: SyncProvider::Trakt,
                op_type,
                item: item.clone(),
                rating,
                attempts: 0,
            });
        }
        if simkl_ok {
            self.state.pending_ops.push(PendingSyncOp {
                op_id: format!("simkl-{}-{ts}", item.id),
                provider: SyncProvider::Simkl,
                op_type,
                item: item.clone(),
                rating,
                attempts: 0,
            });
        }
        if trakt_ok || simkl_ok {
            self.spawn_sync_now(ctx.clone(), true);
        }
    }

    fn spawn_sync_now(&mut self, ctx: egui::Context, silent: bool) {
        if self.sync_in_progress {
            return;
        }
        self.sync_in_progress = true;
        let creds = self.state.credentials.clone();
        let ops = self.state.pending_ops.clone();
        let tx = self.tx.clone();

        std::thread::spawn(move || {
            let mut flushed = Vec::new();
            for op in &ops {
                let ok = match op.provider {
                    SyncProvider::Trakt => {
                        if !creds.trakt_access_token.is_empty() {
                            sync::trakt_execute_sync_op(
                                &creds.trakt_client_id,
                                &creds.trakt_access_token,
                                op,
                            )
                        } else {
                            false
                        }
                    }
                    SyncProvider::Simkl => {
                        if !creds.simkl_access_token.is_empty() {
                            sync::simkl_execute_sync_op(
                                &creds.simkl_client_id,
                                &creds.simkl_access_token,
                                op,
                            )
                        } else {
                            false
                        }
                    }
                };
                if ok {
                    flushed.push(op.op_id.clone());
                }
            }

            let trakt_entries = if !silent && !creds.trakt_access_token.is_empty() {
                Some(sync::trakt_fetch_remote_library(
                    &creds.trakt_client_id,
                    &creds.trakt_access_token,
                ))
            } else {
                None
            };
            let simkl_entries = if !silent && !creds.simkl_access_token.is_empty() {
                Some(sync::simkl_fetch_remote_library(
                    &creds.simkl_client_id,
                    &creds.simkl_access_token,
                ))
            } else {
                None
            };

            let _ = tx.send(BgEvent::RemoteSyncDone {
                trakt_entries,
                simkl_entries,
                flushed_op_ids: flushed,
                silent,
            });
            ctx.request_repaint();
        });
    }

    fn start_device_auth(&mut self, ctx: &egui::Context, provider: SyncProvider) {
        let creds = self.state.credentials.clone();
        let tx = self.tx.clone();
        let ctx_c = ctx.clone();
        std::thread::spawn(move || {
            let res = match provider {
                SyncProvider::Trakt => {
                    if creds.trakt_client_id.trim().is_empty()
                        || creds.trakt_client_secret.trim().is_empty()
                    {
                        Err("Scan the Phone Companion QR first to paste your Trakt Client ID & Secret.".into())
                    } else {
                        sync::trakt_request_device_code(&creds.trakt_client_id)
                    }
                }
                SyncProvider::Simkl => sync::simkl_request_device_code(&creds.simkl_client_id),
            };
            let _ = tx.send(BgEvent::DeviceCodeReady(res));
            ctx_c.request_repaint();
        });
    }

    fn start_trailer_playback(&mut self, ctx: &egui::Context, item: &MediaItem) {
        if let Some(ref yt) = item
            .trailer_youtube_id
            .as_ref()
            .filter(|s| catalog::is_valid_youtube_id(s))
        {
            let q = self.state.settings.default_video_quality;
            if bridge::launch_trailer(item, q, Some(yt)) {
                self.show_toast(
                    format!("Launching {} trailer ({})", item.title, q.label()),
                    None,
                );
            } else {
                self.show_toast("No YouTube / 4K video player installed", None);
            }
            return;
        }
        if self.resolving_trailer {
            return;
        }
        self.resolving_trailer = true;
        self.show_toast("Finding official 2160p 4K trailer…", None);
        let item_c = item.clone();
        let tx = self.tx.clone();
        let ctx_c = ctx.clone();
        std::thread::spawn(move || {
            let yt_id = catalog::resolve_trailer_youtube_id(&item_c);
            let _ = tx.send(BgEvent::TrailerResolved {
                item: item_c,
                yt_id,
            });
            ctx_c.request_repaint();
        });
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                BgEvent::DeckLoaded { reset, page, items } => {
                    self.loading_deck = false;
                    self.deck_page = page;
                    if reset {
                        self.deck = items;
                    } else {
                        let existing: HashSet<String> =
                            self.deck.iter().map(|i| i.id.clone()).collect();
                        for it in items {
                            if !existing.contains(&it.id) {
                                self.deck.push(it);
                            }
                        }
                    }
                    self.filter_deck_in_place();
                }
                BgEvent::ItemEnriched(enriched) => {
                    for c in &mut self.deck {
                        if c.id == enriched.id {
                            *c = enriched.clone();
                        }
                    }
                    if let ModalState::Details(ref mut cur) = self.modal {
                        if cur.id == enriched.id {
                            *cur = enriched;
                        }
                    }
                }
                BgEvent::TrailerResolved { item, yt_id } => {
                    self.resolving_trailer = false;
                    if let Some(ref yt) = yt_id {
                        for c in &mut self.deck {
                            if c.id == item.id {
                                c.trailer_youtube_id = Some(yt.clone());
                            }
                        }
                        if let ModalState::Details(ref mut cur) = self.modal {
                            if cur.id == item.id {
                                cur.trailer_youtube_id = Some(yt.clone());
                            }
                        }
                    }
                    let q = self.state.settings.default_video_quality;
                    if !bridge::launch_trailer(&item, q, yt_id.as_deref()) {
                        self.show_toast("No YouTube / 4K video player installed", None);
                    }
                }
                BgEvent::CompanionCreds(sub) => {
                    self.state.credentials.trakt_client_id = sub.trakt_id;
                    self.state.credentials.trakt_client_secret = sub.trakt_secret;
                    self.state.credentials.simkl_client_id = sub.simkl_id;
                    self.state.credentials.tmdb_api_key = sub.tmdb_key;
                    self.persist();
                    self.show_toast("API keys saved from Phone Companion!", None);
                }
                BgEvent::DeviceCodeReady(res) => match res {
                    Ok(sess) => {
                        self.last_auth_poll = Instant::now();
                        self.auth_poll_inflight = false;
                        self.modal = ModalState::DeviceAuth(sess);
                    }
                    Err(msg) => {
                        self.show_toast(msg, Some(SwipeAction::Skipped));
                    }
                },
                BgEvent::AuthPollOutcome {
                    provider,
                    outcome,
                    username,
                } => {
                    self.auth_poll_inflight = false;
                    match outcome {
                        PollTokenResult::Pending | PollTokenResult::SlowDown => {}
                        PollTokenResult::ExpiredOrDenied(reason) => {
                            self.modal = ModalState::None;
                            self.show_toast(reason, Some(SwipeAction::Skipped));
                        }
                        PollTokenResult::Authorized {
                            access_token,
                            refresh_token,
                        } => {
                            match provider {
                                SyncProvider::Trakt => {
                                    self.state.credentials.trakt_access_token = access_token;
                                    self.state.credentials.trakt_refresh_token = refresh_token;
                                    self.state.credentials.trakt_username =
                                        username.unwrap_or_else(|| "Trakt Connected".into());
                                }
                                SyncProvider::Simkl => {
                                    self.state.credentials.simkl_access_token = access_token;
                                    self.state.credentials.simkl_refresh_token = refresh_token;
                                    self.state.credentials.simkl_username =
                                        username.unwrap_or_else(|| "Simkl Connected".into());
                                }
                            }
                            if let Ok(mut guard) = self.companion.shared_creds.lock() {
                                *guard = self.state.credentials.clone();
                            }
                            self.modal = ModalState::None;
                            self.persist();
                            self.show_toast(
                                format!("{} connected! Syncing library…", provider.name()),
                                Some(SwipeAction::Watched),
                            );
                            self.spawn_sync_now(ctx.clone(), false);
                        }
                    }
                }
                BgEvent::RemoteSyncDone {
                    trakt_entries,
                    simkl_entries,
                    flushed_op_ids,
                    silent,
                } => {
                    self.sync_in_progress = false;
                    if !flushed_op_ids.is_empty() {
                        let set: HashSet<String> = flushed_op_ids.into_iter().collect();
                        self.state.pending_ops.retain(|op| !set.contains(&op.op_id));
                    }
                    let mut imported = 0usize;
                    if let Some(entries) = trakt_entries {
                        self.state.credentials.trakt_last_sync_ms = Utc::now().timestamp_millis();
                        for e in entries {
                            self.state
                                .tracked
                                .entry(e.item.id.clone())
                                .and_modify(|cur| cur.synced_trakt = true)
                                .or_insert_with(|| {
                                    imported += 1;
                                    e
                                });
                        }
                    }
                    if let Some(entries) = simkl_entries {
                        self.state.credentials.simkl_last_sync_ms = Utc::now().timestamp_millis();
                        for e in entries {
                            self.state
                                .tracked
                                .entry(e.item.id.clone())
                                .and_modify(|cur| cur.synced_simkl = true)
                                .or_insert_with(|| {
                                    imported += 1;
                                    e
                                });
                        }
                    }
                    self.filter_deck_in_place();
                    self.persist();
                    if !silent {
                        self.show_toast(
                            format!("Cloud sync complete ({imported} new items imported)"),
                            Some(SwipeAction::ToWatch),
                        );
                    }
                }
            }
        }
    }

    fn poll_active_device_auth(&mut self, ctx: &egui::Context) {
        let ModalState::DeviceAuth(ref sess) = self.modal else {
            return;
        };
        if self.auth_poll_inflight
            || self.last_auth_poll.elapsed() < Duration::from_secs(sess.interval_sec.max(4))
        {
            return;
        }
        self.last_auth_poll = Instant::now();
        self.auth_poll_inflight = true;
        let sess_c = sess.clone();
        let creds = self.state.credentials.clone();
        let tx = self.tx.clone();
        let ctx_c = ctx.clone();

        std::thread::spawn(move || {
            let outcome = match sess_c.provider {
                SyncProvider::Trakt => sync::trakt_poll_device_token(
                    &creds.trakt_client_id,
                    &creds.trakt_client_secret,
                    &sess_c.device_code,
                ),
                SyncProvider::Simkl => {
                    sync::simkl_poll_device_token(&creds.simkl_client_id, &sess_c)
                }
            };
            let username = if let PollTokenResult::Authorized {
                ref access_token, ..
            } = outcome
            {
                match sess_c.provider {
                    SyncProvider::Trakt => {
                        sync::trakt_fetch_username(&creds.trakt_client_id, access_token)
                    }
                    SyncProvider::Simkl => {
                        sync::simkl_fetch_username(&creds.simkl_client_id, access_token)
                    }
                }
            } else {
                None
            };
            let _ = tx.send(BgEvent::AuthPollOutcome {
                provider: sess_c.provider,
                outcome,
                username,
            });
            ctx_c.request_repaint();
        });
    }

    fn qr_texture(&mut self, ctx: &egui::Context, url: &str) -> Option<TextureHandle> {
        if let Some((ref cached_url, ref handle)) = self.qr_cache {
            if cached_url == url {
                return Some(handle.clone());
            }
        }
        let code = QrCode::new(url.as_bytes()).ok()?;
        let w = code.width();
        let quiet = 2usize;
        let scale = 6usize;
        let dim = (w + quiet * 2) * scale;
        let mut pixels = vec![Color32::WHITE; dim * dim];
        for y in 0..w {
            for x in 0..w {
                if code[(x, y)] == qrcode::Color::Dark {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let py = (y + quiet) * scale + dy;
                            let px = (x + quiet) * scale + dx;
                            pixels[py * dim + px] = Color32::from_rgb(8, 12, 16);
                        }
                    }
                }
            }
        }
        let img = ColorImage {
            size: [dim, dim],
            pixels,
        };
        let tex = ctx.load_texture("sway_qr_tex", img, TextureOptions::NEAREST);
        self.qr_cache = Some((url.to_string(), tex.clone()));
        Some(tex)
    }

    fn handle_back_and_menu(&mut self, ctx: &egui::Context) {
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        let jni_back = bridge::take_back_pressed();
        if (esc || jni_back) && self.last_back_at.elapsed() > Duration::from_millis(260) {
            self.last_back_at = Instant::now();
            if !matches!(self.modal, ModalState::None) {
                self.modal = ModalState::None;
                return;
            }
            if !self.top_rail_focused && self.active_tab == NavTab::Deck {
                self.top_rail_focused = true;
                self.top_rail_idx = 0;
                return;
            }
            if self.active_tab != NavTab::Deck {
                self.active_tab = NavTab::Deck;
                self.top_rail_focused = false;
                return;
            }
            bridge::exit_app();
        }

        if bridge::take_menu_pressed() {
            self.modal = if matches!(self.modal, ModalState::Filters) {
                ModalState::None
            } else {
                self.filter_row = 0;
                ModalState::Filters
            };
        }
    }

    // ========================================================================
    // RENDERING: 4K BACKDROP STAGE & TOP RAIL
    // ========================================================================

    fn draw_backdrop_stage(&mut self, ctx: &egui::Context, rect: Rect) {
        let theme = self.state.settings.theme_preset;
        let bg_painter = ctx.layer_painter(egui::LayerId::background());
        bg_painter.rect_filled(rect, CornerRadius::ZERO, theme.bg_deep());

        let active_bg_url = match self.active_tab {
            NavTab::Deck => self
                .deck
                .first()
                .map(|i| i.backdrop_url.clone())
                .unwrap_or_default(),
            NavTab::Collection => self
                .state
                .tracked
                .values()
                .next()
                .map(|e| e.item.backdrop_url.clone())
                .unwrap_or_default(),
            _ => self
                .deck
                .first()
                .map(|i| i.backdrop_url.clone())
                .unwrap_or_default(),
        };

        let kind = if self.state.settings.backdrop_blur_enabled {
            ImgKind::BackdropBlurred
        } else {
            ImgKind::BackdropCrisp
        };

        if let Some(tex) = self.images.get_or_request(&active_bg_url, kind, ctx) {
            bg_painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::from_rgba_unmultiplied(255, 255, 255, 118),
            );
        }

        // Cinema dark vignette + subtle theme glow aura on the right deck stage
        let deep = theme.bg_deep();
        bg_painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(deep.r(), deep.g(), deep.b(), 168),
        );
        let glow = theme.bg_glow();
        let aura_center = Pos2::new(rect.width() * 0.73, rect.height() * 0.48);
        for (r_mult, a) in [(0.52_f32, 34u8), (0.36, 48), (0.22, 62)] {
            bg_painter.circle_filled(
                aura_center,
                rect.height() * r_mult,
                Color32::from_rgba_unmultiplied(glow.r(), glow.g(), glow.b(), a),
            );
        }
    }

    fn draw_top_rail(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let theme = self.state.settings.theme_preset;
        let primary = theme.primary();
        let collection_count = self
            .state
            .tracked
            .values()
            .filter(|e| e.action != SwipeAction::Skipped)
            .count();

        if self.top_rail_focused && matches!(self.modal, ModalState::None) {
            let mut activate = false;
            ctx.input(|i| {
                if i.key_pressed(Key::ArrowRight) {
                    self.top_rail_idx = (self.top_rail_idx + 1).min(5);
                } else if i.key_pressed(Key::ArrowLeft) {
                    self.top_rail_idx = self.top_rail_idx.saturating_sub(1);
                } else if i.key_pressed(Key::ArrowDown) {
                    self.top_rail_focused = false;
                } else if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                    activate = true;
                }
            });
            if activate {
                match self.top_rail_idx {
                    0..=3 => {
                        self.active_tab = NavTab::ALL[self.top_rail_idx];
                        self.top_rail_focused = false;
                    }
                    4 => self.undo_last_swipe(),
                    5 => {
                        self.filter_row = 0;
                        self.modal = ModalState::Filters;
                    }
                    _ => {}
                }
            }
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(40.0);

            // Brand mark + Sway Rust wordmark
            if let Some(ref logo) = self.logo_tex {
                let (lrect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                ui.painter().image(
                    logo.id(),
                    lrect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                ui.painter().rect_stroke(
                    lrect,
                    CornerRadius::same(8),
                    Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 55)),
                    StrokeKind::Inside,
                );
                ui.add_space(6.0);
            }
            ui.label(
                RichText::new("Sway")
                    .font(self.display_font(21.0))
                    .strong()
                    .color(Color32::from_rgb(248, 250, 252)),
            );
            ui.add_space(18.0);

            // 4 Quiet Navigation Tabs
            for (idx, tab) in NavTab::ALL.iter().enumerate() {
                let is_active = self.active_tab == *tab;
                let is_focused = self.top_rail_focused
                    && matches!(self.modal, ModalState::None)
                    && self.top_rail_idx == idx;
                let lbl = if *tab == NavTab::Collection && collection_count > 0 {
                    format!("Collection  {collection_count}")
                } else {
                    tab.label().to_string()
                };

                if Self::glass_pill_button(ui, &lbl, is_active, is_focused, primary) {
                    self.active_tab = *tab;
                    self.top_rail_idx = idx;
                    self.top_rail_focused = false;
                }
                ui.add_space(6.0);
            }

            // Right-aligned Undo & Filters pills
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(40.0);
                let f_focused = self.top_rail_focused
                    && matches!(self.modal, ModalState::None)
                    && self.top_rail_idx == 5;
                if Self::glass_pill_button(
                    ui,
                    &format!("Filters · {}", self.media_type.label()),
                    !self.search_query.is_empty() || self.genre_idx > 0,
                    f_focused,
                    primary,
                ) {
                    self.filter_row = 0;
                    self.modal = ModalState::Filters;
                }
                ui.add_space(8.0);
                let u_focused = self.top_rail_focused
                    && matches!(self.modal, ModalState::None)
                    && self.top_rail_idx == 4;
                if Self::glass_pill_button(ui, "Undo", false, u_focused, theme.secondary()) {
                    self.undo_last_swipe();
                }
            });
        });
    }

    fn glass_pill_button(
        ui: &mut Ui,
        text: &str,
        active: bool,
        focused: bool,
        accent: Color32,
    ) -> bool {
        let pad_x = 15.0;
        let font = FontId::proportional(13.5);
        let galley = ui.painter().layout_no_wrap(
            text.to_string(),
            font.clone(),
            Color32::from_rgb(248, 250, 252),
        );
        let size = Vec2::new(galley.size().x + pad_x * 2.0, 32.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let p = ui.painter();

        let fill = if focused {
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 76)
        } else if active {
            Color32::from_rgba_unmultiplied(255, 255, 255, 28)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 10)
        };
        let stroke = if focused {
            Stroke::new(2.0_f32, accent)
        } else if active {
            Stroke::new(
                1.2_f32,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 150),
            )
        } else {
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 26))
        };

        p.rect_filled(rect, CornerRadius::same(16), fill);
        p.rect_stroke(rect, CornerRadius::same(16), stroke, StrokeKind::Inside);
        let txt_col = if focused || active {
            Color32::WHITE
        } else {
            Color32::from_rgb(198, 208, 222)
        };
        p.text(rect.center(), Align2::CENTER_CENTER, text, font, txt_col);
        resp.clicked()
    }

    // ========================================================================
    // SCREEN 1: ASYMMETRIC 4K SWIPE DECK + 3D PERSPECTIVE POSTER QUAD
    // ========================================================================

    fn draw_deck_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        self.maybe_enrich_top_card(ctx);
        self.prefetch_upcoming_cards(ctx);

        let allow_keys = !self.top_rail_focused && matches!(self.modal, ModalState::None);
        let scheme = self.state.settings.control_scheme;

        if allow_keys {
            let mut swipe_req: Option<SwipeAction> = None;
            let mut open_details = false;
            let mut do_undo = false;

            ctx.input(|i| {
                match scheme {
                    SwipeControlScheme::ThreeWayRemote => {
                        if i.key_pressed(Key::ArrowRight) {
                            swipe_req = Some(SwipeAction::ToWatch);
                        } else if i.key_pressed(Key::ArrowUp) {
                            swipe_req = Some(SwipeAction::Watched);
                        } else if i.key_pressed(Key::ArrowLeft) {
                            swipe_req = Some(SwipeAction::Skipped);
                        } else if i.key_pressed(Key::ArrowDown) {
                            do_undo = true;
                        } else if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                            open_details = true;
                        }
                    }
                    SwipeControlScheme::SplitLeftRight => {
                        if i.key_pressed(Key::ArrowRight) {
                            swipe_req = Some(SwipeAction::Watched);
                        } else if i.key_pressed(Key::ArrowLeft) {
                            swipe_req = Some(SwipeAction::ToWatch);
                        } else if i.key_pressed(Key::ArrowDown) {
                            swipe_req = Some(SwipeAction::Skipped);
                        } else if i.key_pressed(Key::ArrowUp) {
                            self.top_rail_focused = true;
                        } else if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                            open_details = true;
                        }
                    }
                    SwipeControlScheme::ButtonDock => {
                        if i.key_pressed(Key::ArrowRight) {
                            self.deck_btn_idx = (self.deck_btn_idx + 1).min(3);
                        } else if i.key_pressed(Key::ArrowLeft) {
                            self.deck_btn_idx = self.deck_btn_idx.saturating_sub(1);
                        } else if i.key_pressed(Key::ArrowUp) {
                            self.top_rail_focused = true;
                        } else if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                            match self.deck_btn_idx {
                                0 => swipe_req = Some(SwipeAction::Skipped),
                                1 => swipe_req = Some(SwipeAction::ToWatch),
                                2 => swipe_req = Some(SwipeAction::Watched),
                                _ => open_details = true,
                            }
                        }
                    }
                }
            });

            if do_undo {
                self.undo_last_swipe();
            } else if let Some(act) = swipe_req {
                if let Some(top) = self.deck.first().cloned() {
                    self.apply_swipe(ctx, top, act);
                }
            } else if open_details {
                if let Some(top) = self.deck.first().cloned() {
                    self.detail_btn_idx = 0;
                    self.modal = ModalState::Details(top);
                }
            }
        }

        let avail = ui.available_rect_before_wrap();
        if self.deck.is_empty() {
            self.draw_empty_deck(ui, ctx, avail);
            return;
        }

        let top_item = self.deck[0].clone();
        let theme = self.state.settings.theme_preset;

        // Left Editorial Column (52% width) + Right 3D Card Stage (48% width)
        let left_rect = Rect::from_min_max(
            Pos2::new(avail.left() + 48.0, avail.top() + 28.0),
            Pos2::new(avail.left() + avail.width() * 0.54, avail.bottom() - 28.0),
        );
        let right_rect = Rect::from_min_max(
            Pos2::new(avail.left() + avail.width() * 0.55, avail.top() + 10.0),
            Pos2::new(avail.right() - 48.0, avail.bottom() - 24.0),
        );

        // --- LEFT EDITORIAL COLUMN ---
        let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(left_rect));
        left_ui.add_space(22.0);

        // Meta row: Source badge + IMDb gold star + Year + Runtime + Seasons
        left_ui.horizontal_wrapped(|ui| {
            Self::meta_tag(ui, &top_item.source_badge, theme.primary());
            if !top_item.imdb_rating.is_empty() {
                Self::meta_tag(
                    ui,
                    &format!("★ {}", top_item.imdb_rating),
                    Color32::from_rgb(245, 196, 81),
                );
            }
            if !top_item.year.is_empty() {
                Self::meta_tag(ui, &top_item.year, Color32::from_rgb(203, 213, 225));
            }
            if !top_item.runtime.is_empty() {
                Self::meta_tag(ui, &top_item.runtime, Color32::from_rgb(203, 213, 225));
            }
            if let Some(sc) = top_item.seasons_count {
                let s_lbl = if sc == 1 {
                    "1 Season".to_string()
                } else {
                    format!("{sc} Seasons")
                };
                Self::meta_tag(ui, &s_lbl, Color32::from_rgb(203, 213, 225));
            }
        });

        left_ui.add_space(14.0);

        // Cinema Display Title
        let title_len = top_item.title.chars().count();
        let title_sz = if title_len > 30 {
            34.0
        } else if title_len > 20 {
            40.0
        } else {
            46.0
        };
        left_ui.label(
            RichText::new(&top_item.title)
                .font(self.display_font(title_sz))
                .strong()
                .color(Color32::from_rgb(248, 250, 252)),
        );

        left_ui.add_space(10.0);

        // Genres row
        if !top_item.genres.is_empty() {
            left_ui.horizontal_wrapped(|ui| {
                for g in top_item.genres.iter().take(4) {
                    egui::Frame::NONE
                        .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 14))
                        .stroke(Stroke::new(
                            1.0_f32,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 28),
                        ))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(g)
                                    .font(FontId::proportional(12.5))
                                    .color(Color32::from_rgb(226, 232, 240)),
                            );
                        });
                }
            });
            left_ui.add_space(14.0);
        }

        // Synopsis (clean, max ~320 chars so it breathes on 4K TV)
        let synopsis = if top_item.overview.chars().count() > 310 {
            format!(
                "{}…",
                top_item.overview.chars().take(308).collect::<String>()
            )
        } else if top_item.overview.is_empty() {
            "Select Details & 4K Trailer to explore full cast, ratings, and official 2160p preview."
                .to_string()
        } else {
            top_item.overview.clone()
        };
        left_ui.label(
            RichText::new(synopsis)
                .font(FontId::proportional(15.5))
                .color(Color32::from_rgb(208, 217, 230)),
        );

        if !top_item.cast.is_empty() {
            left_ui.add_space(12.0);
            left_ui.label(
                RichText::new(format!("Starring  {}", top_item.cast.join(", ")))
                    .font(FontId::proportional(13.0))
                    .color(Color32::from_rgb(148, 163, 184)),
            );
        }
        if let Some(aw) = top_item.awards.as_deref().filter(|s| !s.is_empty()) {
            left_ui.add_space(6.0);
            left_ui.label(
                RichText::new(aw)
                    .font(FontId::proportional(12.5))
                    .color(theme.secondary()),
            );
        }

        left_ui.add_space(26.0);

        // Action Pills Dock (shows live remote shortcuts or focusable buttons)
        let mut clicked_action: Option<u8> = None;
        left_ui.horizontal_wrapped(|ui| {
            let (k_pass, k_watch, k_seen) = match scheme {
                SwipeControlScheme::ThreeWayRemote => ("← ", "→ ", "↑ "),
                SwipeControlScheme::SplitLeftRight => ("↓ ", "← ", "→ "),
                SwipeControlScheme::ButtonDock => ("", "", ""),
            };
            let dock_mode = scheme == SwipeControlScheme::ButtonDock && allow_keys;
            if Self::action_dock_pill(
                ui,
                &format!("{k_pass}Pass"),
                theme.pass_color(),
                dock_mode && self.deck_btn_idx == 0,
            ) {
                clicked_action = Some(0);
            }
            ui.add_space(6.0);
            if Self::action_dock_pill(
                ui,
                &format!("{k_watch}Watchlist"),
                theme.to_watch_color(),
                dock_mode && self.deck_btn_idx == 1,
            ) {
                clicked_action = Some(1);
            }
            ui.add_space(6.0);
            if Self::action_dock_pill(
                ui,
                &format!("{k_seen}Watched"),
                theme.watched_color(),
                dock_mode && self.deck_btn_idx == 2,
            ) {
                clicked_action = Some(2);
            }
            ui.add_space(6.0);
            if Self::action_dock_pill(
                ui,
                "OK · Details & 2160p",
                theme.primary(),
                dock_mode && self.deck_btn_idx == 3,
            ) {
                clicked_action = Some(3);
            }
        });

        left_ui.add_space(12.0);
        left_ui.label(
            RichText::new(if scheme == SwipeControlScheme::ThreeWayRemote {
                "Press Up on Watchlist/Top Rail: press Back once to jump to Top Rail · Down rewinds last card"
            } else {
                scheme.description()
            })
            .font(FontId::proportional(11.5))
            .color(Color32::from_rgb(125, 138, 158)),
        );

        if let Some(act_id) = clicked_action {
            match act_id {
                0 => self.apply_swipe(ctx, top_item.clone(), SwipeAction::Skipped),
                1 => self.apply_swipe(ctx, top_item.clone(), SwipeAction::ToWatch),
                2 => self.apply_swipe(ctx, top_item.clone(), SwipeAction::Watched),
                _ => {
                    self.detail_btn_idx = 0;
                    self.modal = ModalState::Details(top_item.clone());
                }
            }
        }

        // --- RIGHT 3D PERSPECTIVE POSTER DECK STAGE ---
        self.draw_poster_deck_stage(ui, ctx, right_rect, allow_keys);
    }

    fn meta_tag(ui: &mut Ui, text: &str, color: Color32) {
        egui::Frame::NONE
            .fill(Color32::from_rgba_unmultiplied(
                color.r(),
                color.g(),
                color.b(),
                26,
            ))
            .stroke(Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90),
            ))
            .corner_radius(CornerRadius::same(9))
            .inner_margin(egui::Margin::symmetric(9, 4))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(text)
                        .font(FontId::proportional(12.0))
                        .strong()
                        .color(color),
                );
            });
    }

    fn action_dock_pill(ui: &mut Ui, label: &str, color: Color32, focused: bool) -> bool {
        let font = FontId::proportional(13.5);
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE);
        let size = Vec2::new(galley.size().x + 24.0, 36.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let p = ui.painter();

        let fill = if focused {
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 88)
        } else {
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 28)
        };
        let stroke = if focused {
            Stroke::new(2.2_f32, color)
        } else {
            Stroke::new(
                1.2_f32,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 110),
            )
        };
        p.rect_filled(rect, CornerRadius::same(12), fill);
        p.rect_stroke(rect, CornerRadius::same(12), stroke, StrokeKind::Inside);
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            font,
            Color32::from_rgb(248, 250, 252),
        );
        resp.clicked()
    }

    fn draw_poster_deck_stage(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        stage_rect: Rect,
        deck_focused: bool,
    ) {
        let theme = self.state.settings.theme_preset;
        let card_h = (stage_rect.height() * 0.84).clamp(280.0, 420.0);
        let card_w = card_h * 0.675;
        let base_center = Pos2::new(
            stage_rect.center().x - 12.0,
            stage_rect.center().y + 4.0,
        );

        // Advance progress of any active fly-out or rewind animation
        let fly_prog = self.fly_anim.as_ref().map(|a| {
            let p = (a.started_at.elapsed().as_secs_f32() / a.duration).clamp(0.0, 1.0);
            ctx.request_repaint();
            p
        });
        if fly_prog.is_some_and(|p| p >= 1.0) {
            self.fly_anim = None;
        }

        let rew_prog = self.rewind_anim.as_ref().map(|a| {
            let p = (a.started_at.elapsed().as_secs_f32() / a.duration).clamp(0.0, 1.0);
            ctx.request_repaint();
            p
        });
        if rew_prog.is_some_and(|p| p >= 1.0) {
            self.rewind_anim = None;
        }

        let promote = fly_prog
            .map(|p| 1.0 - (1.0 - p).powi(3))
            .unwrap_or(0.0);

        // Draw back cards (#2 and #1) in stacked 3D depth
        for depth in (1..=2).rev() {
            if let Some(back_item) = self.deck.get(depth).cloned() {
                let d_eff = (depth as f32 - promote).max(0.0);
                let scale = 1.0 - d_eff * 0.065;
                let offset = Vec2::new(d_eff * 26.0, d_eff * 10.0);
                let c = base_center + offset;
                let sz = Vec2::new(card_w * scale, card_h * scale);
                let r = Rect::from_center_size(c, sz);
                let tex_id = self
                    .images
                    .get_or_request(&back_item.poster_url, ImgKind::Poster, ctx)
                    .map(|t| t.id());
                let p = ui.painter();
                p.rect_filled(
                    r.translate(Vec2::new(0.0, 8.0)),
                    CornerRadius::same(18),
                    Color32::from_rgba_unmultiplied(0, 0, 0, 115),
                );
                p.rect_filled(r, CornerRadius::same(18), theme.card_tint());
                if let Some(tid) = tex_id {
                    let dim = (215.0 - d_eff * 45.0) as u8;
                    p.image(
                        tid,
                        r,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::from_rgba_unmultiplied(dim, dim, dim, 255),
                    );
                }
                p.rect_stroke(
                    r,
                    CornerRadius::same(18),
                    Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 36)),
                    StrokeKind::Inside,
                );
            }
        }

        // Front Hero Card (#0) with resting liquid-glass float + rewind spring
        if let Some(front) = self.deck.first().cloned() {
            let t_sec = self.start_time.elapsed().as_secs_f32();
            let float_y = if deck_focused {
                ctx.request_repaint();
                (t_sec * 2.0).sin() * 3.2
            } else {
                0.0
            };

            let (rew_dx, rew_tilt) = if let Some(rp) = rew_prog {
                let inv = (1.0 - rp).powi(3);
                (-110.0 * inv, -0.18 * inv)
            } else {
                (0.0, 0.0)
            };

            let c = Pos2::new(base_center.x + rew_dx, base_center.y - float_y);
            let card_rect = Rect::from_center_size(c, Vec2::new(card_w, card_h));
            let resp = ui.interact(card_rect, egui::Id::new("front_hero_card"), Sense::click());
            if resp.clicked() {
                self.detail_btn_idx = 0;
                self.modal = ModalState::Details(front.clone());
            }

            let tex_id = self
                .images
                .get_or_request(&front.poster_url, ImgKind::Poster, ctx)
                .map(|t| t.id());
            let p = ui.painter();

            // Deep grounded shadow + optional theme card glow
            if self.state.settings.card_shadow_glow {
                let pr = theme.primary();
                p.rect_filled(
                    card_rect.expand(8.0),
                    CornerRadius::same(24),
                    Color32::from_rgba_unmultiplied(pr.r(), pr.g(), pr.b(), 34),
                );
            }
            p.rect_filled(
                Rect::from_center_size(
                    Pos2::new(base_center.x + rew_dx, base_center.y + 12.0),
                    Vec2::new(card_w * 0.96, card_h * 0.98),
                ),
                CornerRadius::same(20),
                Color32::from_rgba_unmultiplied(0, 0, 0, 155),
            );

            Self::draw_perspective_card(
                p,
                c,
                Vec2::new(card_w, card_h),
                rew_tilt,
                0.0,
                tex_id,
                theme.card_tint(),
                &front.title,
                if deck_focused {
                    theme.primary()
                } else {
                    Color32::from_rgba_unmultiplied(255, 255, 255, 70)
                },
                255,
            );

            // Orbital specular glass rim glint along the top/side edge while resting
            if deck_focused {
                let phase = (t_sec * 0.55).fract();
                let shimmer_x = card_rect.left() + 24.0 + phase * (card_w - 48.0);
                p.line_segment(
                    [
                        Pos2::new((shimmer_x - 28.0).max(card_rect.left() + 14.0), card_rect.top() + 1.0),
                        Pos2::new((shimmer_x + 28.0).min(card_rect.right() - 14.0), card_rect.top() + 1.0),
                    ],
                    Stroke::new(2.4_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 210)),
                );
            }
        }

        // Flying-out swiped card on top with true 3D perspective tilt + glowing stamp!
        if let Some(ref fly) = self.fly_anim {
            let raw_p = (fly.started_at.elapsed().as_secs_f32() / fly.duration).clamp(0.0, 1.0);
            let ease = 1.0 - (1.0 - raw_p).powi(3);
            let (dir_v, yaw_tilt, pitch_tilt, stamp_txt, stamp_col) = match fly.action {
                SwipeAction::ToWatch => (
                    Vec2::new(280.0, -18.0),
                    0.34,
                    0.0,
                    "WATCHLIST",
                    theme.to_watch_color(),
                ),
                SwipeAction::Watched => (
                    Vec2::new(0.0, -240.0),
                    0.0,
                    0.28,
                    "WATCHED ✓",
                    theme.watched_color(),
                ),
                SwipeAction::Skipped => (
                    Vec2::new(-280.0, 18.0),
                    -0.34,
                    0.0,
                    "PASS",
                    theme.pass_color(),
                ),
            };
            let fly_c = base_center + dir_v * ease;
            let alpha = ((1.0 - raw_p * 0.88) * 255.0) as u8;
            let tex_id = self
                .images
                .get_or_request(&fly.item.poster_url, ImgKind::Poster, ctx)
                .map(|t| t.id());
            let p = ui.painter();
            Self::draw_perspective_card(
                p,
                fly_c,
                Vec2::new(card_w, card_h),
                yaw_tilt * ease,
                pitch_tilt * ease,
                tex_id,
                theme.card_tint(),
                &fly.item.title,
                stamp_col,
                alpha,
            );

            // Neon stamp pill across flying card
            let stamp_rect = Rect::from_center_size(fly_c, Vec2::new(156.0, 42.0));
            p.rect_filled(
                stamp_rect,
                CornerRadius::same(12),
                Color32::from_rgba_unmultiplied(8, 12, 16, alpha.min(220)),
            );
            p.rect_stroke(
                stamp_rect,
                CornerRadius::same(12),
                Stroke::new(2.4_f32, stamp_col),
                StrokeKind::Inside,
            );
            p.text(
                fly_c,
                Align2::CENTER_CENTER,
                stamp_txt,
                FontId::proportional(16.5),
                stamp_col,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_perspective_card(
        p: &egui::Painter,
        center: Pos2,
        size: Vec2,
        yaw: f32,
        pitch: f32,
        tex_id: Option<egui::TextureId>,
        fallback_fill: Color32,
        fallback_title: &str,
        border_col: Color32,
        alpha: u8,
    ) {
        let hw = size.x * 0.5;
        let hh = size.y * 0.5;
        let rot = yaw * 0.35;
        let (sr, cr) = rot.sin_cos();

        // 3D trapezoidal foreshortening on left/right (yaw) and top/bottom (pitch)
        let right_scale = 1.0 - yaw * 0.16;
        let left_scale = 1.0 + yaw * 0.16;
        let top_scale = 1.0 - pitch * 0.14;
        let bot_scale = 1.0 + pitch * 0.14;

        let local_pts = [
            Vec2::new(-hw * top_scale, -hh * left_scale),
            Vec2::new(hw * top_scale, -hh * right_scale),
            Vec2::new(hw * bot_scale, hh * right_scale),
            Vec2::new(-hw * bot_scale, hh * left_scale),
        ];
        let pts: Vec<Pos2> = local_pts
            .iter()
            .map(|v| Pos2::new(center.x + v.x * cr - v.y * sr, center.y + v.x * sr + v.y * cr))
            .collect();

        let tint = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
        if let Some(tid) = tex_id {
            let mut mesh = Mesh::with_texture(tid);
            let uvs = [
                Pos2::new(0.0, 0.0),
                Pos2::new(1.0, 0.0),
                Pos2::new(1.0, 1.0),
                Pos2::new(0.0, 1.0),
            ];
            for i in 0..4 {
                mesh.colored_vertex(pts[i], tint);
                mesh.vertices[i].uv = uvs[i];
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            p.add(egui::Shape::mesh(mesh));
        } else {
            p.add(egui::Shape::convex_polygon(
                pts.clone(),
                Color32::from_rgba_unmultiplied(
                    fallback_fill.r(),
                    fallback_fill.g(),
                    fallback_fill.b(),
                    alpha,
                ),
                Stroke::NONE,
            ));
            p.text(
                center,
                Align2::CENTER_CENTER,
                fallback_title,
                FontId::proportional(18.0),
                Color32::WHITE,
            );
        }

        let b_col = Color32::from_rgba_unmultiplied(
            border_col.r(),
            border_col.g(),
            border_col.b(),
            alpha,
        );
        p.add(egui::Shape::closed_line(pts, Stroke::new(2.2_f32, b_col)));
    }

    fn draw_empty_deck(&mut self, ui: &mut Ui, ctx: &egui::Context, avail: Rect) {
        let theme = self.state.settings.theme_preset;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(avail));
        child.vertical_centered(|ui| {
            ui.add_space(avail.height() * 0.28);
            ui.label(
                RichText::new(if self.loading_deck {
                    "Loading 4K Discovery Deck…"
                } else {
                    "You've seen everything in this filter"
                })
                .font(self.display_font(28.0))
                .strong()
                .color(Color32::WHITE),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new("Reset passed titles or switch category/genre to load fresh picks.")
                    .font(FontId::proportional(14.5))
                    .color(Color32::from_rgb(165, 178, 196)),
            );
            ui.add_space(20.0);
            if Self::glass_pill_button(ui, "Refresh & Reset Passed", true, true, theme.primary())
                || (!self.top_rail_focused
                    && matches!(self.modal, ModalState::None)
                    && ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space)))
            {
                self.state
                    .tracked
                    .retain(|_, v| v.action != SwipeAction::Skipped);
                self.persist();
                self.spawn_fetch_deck(ctx.clone(), true, 0);
            }
        });
    }

    // ========================================================================
    // SCREEN 2: COLLECTION (WATCHLIST & WATCHED)
    // ========================================================================

    fn draw_collection_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let theme = self.state.settings.theme_preset;
        let primary = theme.primary();

        let mut entries: Vec<TrackedEntry> = self
            .state
            .tracked
            .values()
            .filter(|e| {
                if self.collection_subtab == 0 {
                    e.action == SwipeAction::ToWatch
                } else {
                    e.action == SwipeAction::Watched
                }
            })
            .cloned()
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.updated_at_ms));

        if self.collection_focus >= entries.len() && !entries.is_empty() {
            self.collection_focus = entries.len() - 1;
        }

        let allow_keys = !self.top_rail_focused && matches!(self.modal, ModalState::None);
        let mut trigger_act: Option<(usize, usize)> = None;
        if allow_keys {
            ctx.input(|i| {
                if i.key_pressed(Key::ArrowDown) {
                    if !entries.is_empty() {
                        self.collection_focus = (self.collection_focus + 1).min(entries.len() - 1);
                    }
                } else if i.key_pressed(Key::ArrowUp) {
                    if self.collection_focus == 0 {
                        self.top_rail_focused = true;
                    } else {
                        self.collection_focus -= 1;
                    }
                } else if i.key_pressed(Key::ArrowRight) {
                    if self.collection_act_idx < 2 {
                        self.collection_act_idx += 1;
                    } else {
                        self.collection_subtab = 1;
                        self.collection_focus = 0;
                    }
                } else if i.key_pressed(Key::ArrowLeft) {
                    if self.collection_act_idx > 0 {
                        self.collection_act_idx -= 1;
                    } else {
                        self.collection_subtab = 0;
                        self.collection_focus = 0;
                    }
                } else if (i.key_pressed(Key::Enter) || i.key_pressed(Key::Space))
                    && !entries.is_empty()
                {
                    trigger_act = Some((self.collection_focus, self.collection_act_idx));
                }
            });
        }

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(48.0);
            let wl_cnt = self
                .state
                .tracked
                .values()
                .filter(|e| e.action == SwipeAction::ToWatch)
                .count();
            let wt_cnt = self
                .state
                .tracked
                .values()
                .filter(|e| e.action == SwipeAction::Watched)
                .count();

            if Self::glass_pill_button(
                ui,
                &format!("Watchlist ({wl_cnt})"),
                self.collection_subtab == 0,
                false,
                theme.to_watch_color(),
            ) {
                self.collection_subtab = 0;
                self.collection_focus = 0;
            }
            ui.add_space(8.0);
            if Self::glass_pill_button(
                ui,
                &format!("Watched ({wt_cnt})"),
                self.collection_subtab == 1,
                false,
                theme.watched_color(),
            ) {
                self.collection_subtab = 1;
                self.collection_focus = 0;
            }
        });

        ui.add_space(14.0);
        if entries.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(90.0);
                ui.label(
                    RichText::new(if self.collection_subtab == 0 {
                        "Your Watchlist is empty"
                    } else {
                        "No Watched titles yet"
                    })
                    .font(self.display_font(26.0))
                    .color(Color32::WHITE),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Swipe cards on the Deck or sync your Trakt / Simkl account.")
                        .font(FontId::proportional(14.0))
                        .color(Color32::from_rgb(160, 174, 192)),
                );
            });
            return;
        }

        ScrollArea::vertical()
            .id_salt("collection_scroll")
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                for (idx, entry) in entries.iter().enumerate() {
                    let row_focused = allow_keys && self.collection_focus == idx;
                    ui.horizontal(|ui| {
                        ui.add_space(48.0);
                        let row_w = ui.available_width() - 96.0;
                        let (rect, resp) =
                            ui.allocate_exact_size(Vec2::new(row_w, 72.0), Sense::click());
                        if row_focused {
                            resp.scroll_to_me(Some(Align::Center));
                        }
                        let p = ui.painter();
                        let fill = if row_focused {
                            Color32::from_rgba_unmultiplied(primary.r(), primary.g(), primary.b(), 38)
                        } else {
                            Color32::from_rgba_unmultiplied(255, 255, 255, 12)
                        };
                        let stroke = if row_focused {
                            Stroke::new(2.0_f32, primary)
                        } else {
                            Stroke::new(
                                1.0_f32,
                                Color32::from_rgba_unmultiplied(255, 255, 255, 22),
                            )
                        };
                        p.rect_filled(rect, CornerRadius::same(14), fill);
                        p.rect_stroke(rect, CornerRadius::same(14), stroke, StrokeKind::Inside);

                        // Mini poster
                        let poster_r = Rect::from_min_size(
                            Pos2::new(rect.left() + 12.0, rect.top() + 8.0),
                            Vec2::new(38.0, 56.0),
                        );
                        if let Some(tex) =
                            self.images
                                .get_or_request(&entry.item.poster_url, ImgKind::Poster, ctx)
                        {
                            p.image(
                                tex.id(),
                                poster_r,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            p.rect_filled(poster_r, CornerRadius::same(6), theme.card_tint());
                        }

                        p.text(
                            Pos2::new(rect.left() + 62.0, rect.top() + 24.0),
                            Align2::LEFT_CENTER,
                            &entry.item.title,
                            FontId::proportional(16.0),
                            Color32::WHITE,
                        );
                        let mut sub = format!(
                            "{} · {}",
                            entry.item.media_type.label(),
                            entry.item.year
                        );
                        if let Some(r) = entry.user_rating {
                            sub.push_str(&format!(" · Rated ★ {r}/10"));
                        }
                        if entry.synced_trakt {
                            sub.push_str(" · Trakt ✓");
                        }
                        if entry.synced_simkl {
                            sub.push_str(" · Simkl ✓");
                        }
                        p.text(
                            Pos2::new(rect.left() + 62.0, rect.top() + 47.0),
                            Align2::LEFT_CENTER,
                            sub,
                            FontId::proportional(12.5),
                            Color32::from_rgb(168, 182, 200),
                        );

                        // Right action pills
                        let labels = [
                            "Details & 4K",
                            if self.collection_subtab == 0 {
                                "Mark Watched"
                            } else {
                                "Move to Watchlist"
                            },
                            "Remove",
                        ];
                        let mut x_right = rect.right() - 14.0;
                        for (a_idx, a_lbl) in labels.iter().enumerate().rev() {
                            let w = 122.0;
                            let btn_r = Rect::from_min_max(
                                Pos2::new(x_right - w, rect.center().y - 15.0),
                                Pos2::new(x_right, rect.center().y + 15.0),
                            );
                            let sub_f = row_focused && self.collection_act_idx == a_idx;
                            let b_col = if a_idx == 2 {
                                theme.pass_color()
                            } else {
                                primary
                            };
                            p.rect_filled(
                                btn_r,
                                CornerRadius::same(9),
                                if sub_f {
                                    Color32::from_rgba_unmultiplied(
                                        b_col.r(),
                                        b_col.g(),
                                        b_col.b(),
                                        92,
                                    )
                                } else {
                                    Color32::from_rgba_unmultiplied(255, 255, 255, 16)
                                },
                            );
                            if sub_f {
                                p.rect_stroke(
                                    btn_r,
                                    CornerRadius::same(9),
                                    Stroke::new(1.8_f32, b_col),
                                    StrokeKind::Inside,
                                );
                            }
                            p.text(
                                btn_r.center(),
                                Align2::CENTER_CENTER,
                                *a_lbl,
                                FontId::proportional(12.0),
                                Color32::WHITE,
                            );
                            if ui
                                .interact(
                                    btn_r,
                                    egui::Id::new(("col_act", idx, a_idx)),
                                    Sense::click(),
                                )
                                .clicked()
                            {
                                trigger_act = Some((idx, a_idx));
                            }
                            x_right -= w + 8.0;
                        }

                        if resp.clicked() {
                            trigger_act = Some((idx, 0));
                        }
                    });
                    ui.add_space(7.0);
                }
                ui.add_space(28.0);
            });

        if let Some((e_idx, a_idx)) = trigger_act {
            if let Some(target) = entries.get(e_idx).cloned() {
                match a_idx {
                    0 => {
                        self.detail_btn_idx = 0;
                        self.modal = ModalState::Details(target.item);
                    }
                    1 => {
                        let new_act = if target.action == SwipeAction::ToWatch {
                            SwipeAction::Watched
                        } else {
                            SwipeAction::ToWatch
                        };
                        if let Some(cur) = self.state.tracked.get_mut(&target.item.id) {
                            cur.action = new_act;
                            cur.updated_at_ms = Utc::now().timestamp_millis();
                        }
                        let op = if new_act == SwipeAction::Watched {
                            SyncOpType::AddWatched
                        } else {
                            SyncOpType::AddWatchlist
                        };
                        self.enqueue_sync_op(&target.item, op, None, ctx);
                        self.persist();
                        self.show_toast(
                            format!("Moved \"{}\" to {}", target.item.title, new_act.label()),
                            Some(new_act),
                        );
                    }
                    2 => {
                        self.state.tracked.remove(&target.item.id);
                        let rem_op = if target.action == SwipeAction::Watched {
                            SyncOpType::RemoveWatched
                        } else {
                            SyncOpType::RemoveWatchlist
                        };
                        self.enqueue_sync_op(&target.item, rem_op, None, ctx);
                        self.persist();
                        self.show_toast(format!("Removed \"{}\"", target.item.title), None);
                    }
                    _ => {}
                }
            }
        }
    }

    // ========================================================================
    // SCREEN 3: SYNC HUB (TRAKT + SIMKL + PHONE COMPANION QR)
    // ========================================================================

    fn draw_sync_hub_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let theme = self.state.settings.theme_preset;
        let primary = theme.primary();
        let allow_keys = !self.top_rail_focused && matches!(self.modal, ModalState::None);

        let mut act: Option<usize> = None;
        if allow_keys {
            ctx.input(|i| {
                if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown) {
                    self.sync_focus = (self.sync_focus + 1).min(3);
                } else if i.key_pressed(Key::ArrowLeft) {
                    self.sync_focus = self.sync_focus.saturating_sub(1);
                } else if i.key_pressed(Key::ArrowUp) {
                    if self.sync_focus == 0 {
                        self.top_rail_focused = true;
                    } else {
                        self.sync_focus -= 1;
                    }
                } else if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                    act = Some(self.sync_focus);
                }
            });
        }

        let avail = ui.available_rect_before_wrap();
        let left_r = Rect::from_min_max(
            Pos2::new(avail.left() + 48.0, avail.top() + 20.0),
            Pos2::new(avail.left() + avail.width() * 0.60, avail.bottom() - 24.0),
        );
        let right_r = Rect::from_min_max(
            Pos2::new(avail.left() + avail.width() * 0.62, avail.top() + 20.0),
            Pos2::new(avail.right() - 48.0, avail.bottom() - 24.0),
        );

        let mut l_ui = ui.new_child(egui::UiBuilder::new().max_rect(left_r));
        l_ui.label(
            RichText::new("Cloud Sync Hub")
                .font(self.display_font(28.0))
                .strong()
                .color(Color32::WHITE),
        );
        l_ui.label(
            RichText::new(format!(
                "Bidirectional Trakt & Simkl sync · {} queued ops",
                self.state.pending_ops.len()
            ))
            .font(FontId::proportional(13.5))
            .color(Color32::from_rgb(165, 178, 196)),
        );
        l_ui.add_space(16.0);

        // Trakt Card
        let trakt_conn = !self.state.credentials.trakt_access_token.is_empty();
        let trakt_keys = !self.state.credentials.trakt_client_id.is_empty()
            && !self.state.credentials.trakt_client_secret.is_empty();
        Self::provider_card(
            &mut l_ui,
            "Trakt",
            if trakt_conn {
                format!("Connected as {}", self.state.credentials.trakt_username)
            } else if trakt_keys {
                "Client keys configured · Ready to authorize".into()
            } else {
                "Scan the Phone Companion QR on the right to paste Trakt keys".into()
            },
            trakt_conn,
            primary,
            allow_keys && self.sync_focus == 0,
            allow_keys && self.sync_focus == 1,
            &mut act,
            0,
            1,
        );

        l_ui.add_space(14.0);

        // Simkl Card
        let simkl_conn = !self.state.credentials.simkl_access_token.is_empty();
        let simkl_keys = !self.state.credentials.simkl_client_id.is_empty();
        Self::provider_card(
            &mut l_ui,
            "Simkl",
            if simkl_conn {
                format!("Connected as {}", self.state.credentials.simkl_username)
            } else if simkl_keys {
                "Client ID configured · Ready to authorize".into()
            } else {
                "Scan the Phone Companion QR on the right to paste Simkl Client ID".into()
            },
            simkl_conn,
            theme.secondary(),
            allow_keys && self.sync_focus == 2,
            allow_keys && self.sync_focus == 3,
            &mut act,
            2,
            3,
        );

        // Right Phone Companion QR Card
        let comp_url = self
            .companion
            .current_url()
            .unwrap_or_else(|| "http://192.168.1.200:8766".into());
        let qr_tex = self.qr_texture(ctx, &comp_url);

        let mut r_ui = ui.new_child(egui::UiBuilder::new().max_rect(right_r));
        egui::Frame::NONE
            .fill(Color32::from_rgba_unmultiplied(12, 20, 24, 210))
            .stroke(Stroke::new(
                1.2_f32,
                Color32::from_rgba_unmultiplied(primary.r(), primary.g(), primary.b(), 95),
            ))
            .corner_radius(CornerRadius::same(22))
            .inner_margin(egui::Margin::same(22))
            .show(&mut r_ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Phone Companion Setup")
                            .font(self.display_font(20.0))
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(
                            "Scan with your phone on home Wi-Fi to paste Trakt or Simkl keys in seconds.",
                        )
                        .font(FontId::proportional(12.5))
                        .color(Color32::from_rgb(172, 186, 204)),
                    );
                    ui.add_space(16.0);
                    if let Some(tex) = qr_tex {
                        let (qr_r, _) = ui.allocate_exact_size(Vec2::splat(168.0), Sense::hover());
                        ui.painter().rect_filled(
                            qr_r.expand(6.0),
                            CornerRadius::same(14),
                            Color32::WHITE,
                        );
                        ui.painter().image(
                            tex.id(),
                            qr_r,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(&comp_url)
                            .font(FontId::proportional(13.5))
                            .strong()
                            .color(primary),
                    );
                });
            });

        if let Some(code_act) = act {
            match code_act {
                0 => {
                    if trakt_conn {
                        self.state.credentials.trakt_access_token.clear();
                        self.state.credentials.trakt_refresh_token.clear();
                        self.state.credentials.trakt_username.clear();
                        self.persist();
                        self.show_toast("Disconnected Trakt", None);
                    } else {
                        self.start_device_auth(ctx, SyncProvider::Trakt);
                    }
                }
                1 => self.spawn_sync_now(ctx.clone(), false),
                2 => {
                    if simkl_conn {
                        self.state.credentials.simkl_access_token.clear();
                        self.state.credentials.simkl_refresh_token.clear();
                        self.state.credentials.simkl_username.clear();
                        self.persist();
                        self.show_toast("Disconnected Simkl", None);
                    } else {
                        self.start_device_auth(ctx, SyncProvider::Simkl);
                    }
                }
                3 => self.spawn_sync_now(ctx.clone(), false),
                _ => {}
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn provider_card(
        ui: &mut Ui,
        name: &str,
        status: String,
        connected: bool,
        accent: Color32,
        f_btn1: bool,
        f_btn2: bool,
        out_act: &mut Option<usize>,
        id1: usize,
        id2: usize,
    ) {
        egui::Frame::NONE
            .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 14))
            .stroke(Stroke::new(
                1.2_f32,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 80),
            ))
            .corner_radius(CornerRadius::same(18))
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let dot_c = if connected {
                        Color32::from_rgb(52, 211, 153)
                    } else {
                        Color32::from_rgb(148, 163, 184)
                    };
                    let (dr, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                    ui.painter().circle_filled(dr.center(), 4.5, dot_c);
                    ui.label(
                        RichText::new(name)
                            .font(FontId::proportional(18.5))
                            .strong()
                            .color(Color32::WHITE),
                    );
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(status)
                        .font(FontId::proportional(13.0))
                        .color(Color32::from_rgb(182, 195, 212)),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let lbl1 = if connected {
                        "Disconnect"
                    } else {
                        "Connect with TV Code"
                    };
                    if Self::action_dock_pill(ui, lbl1, accent, f_btn1) {
                        *out_act = Some(id1);
                    }
                    ui.add_space(8.0);
                    if Self::action_dock_pill(ui, "Sync Library Now", accent, f_btn2) {
                        *out_act = Some(id2);
                    }
                });
            });
    }

    // ========================================================================
    // SCREEN 4: THEMES & 4K LIQUID-GLASS SETTINGS
    // ========================================================================

    fn draw_themes_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let theme = self.state.settings.theme_preset;
        let primary = theme.primary();
        let allow_keys = !self.top_rail_focused && matches!(self.modal, ModalState::None);
        const TOTAL_ITEMS: usize = 17; // 0..6 themes + 11 settings rows

        let mut chosen: Option<usize> = None;
        if allow_keys {
            ctx.input(|i| {
                if i.key_pressed(Key::ArrowDown) {
                    if self.settings_focus < 3 {
                        self.settings_focus += 3;
                    } else {
                        self.settings_focus = (self.settings_focus + 1).min(TOTAL_ITEMS - 1);
                    }
                } else if i.key_pressed(Key::ArrowUp) {
                    if self.settings_focus == 0 {
                        self.top_rail_focused = true;
                    } else if (3..6).contains(&self.settings_focus) {
                        self.settings_focus -= 3;
                    } else {
                        self.settings_focus = self.settings_focus.saturating_sub(1);
                    }
                } else if i.key_pressed(Key::ArrowRight) && self.settings_focus < 5 {
                    self.settings_focus += 1;
                } else if i.key_pressed(Key::ArrowLeft)
                    && self.settings_focus > 0
                    && self.settings_focus < 6
                {
                    self.settings_focus -= 1;
                } else if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                    chosen = Some(self.settings_focus);
                }
            });
        }

        ScrollArea::vertical()
            .id_salt("themes_settings_scroll")
            .show(ui, |ui| {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space(48.0);
                    ui.label(
                        RichText::new("Liquid-Glass Themes & 4K Preferences")
                            .font(self.display_font(26.0))
                            .strong()
                            .color(Color32::WHITE),
                    );
                });
                ui.add_space(12.0);

                // 6 Dual-Tone Glass Theme Cards (2 rows of 3)
                for row in 0..2 {
                    ui.horizontal(|ui| {
                        ui.add_space(48.0);
                        let card_w = (ui.available_width() - 96.0 - 24.0) / 3.0;
                        for col in 0..3 {
                            let idx = row * 3 + col;
                            let preset = ThemePreset::ALL[idx];
                            let is_sel = self.state.settings.theme_preset == preset;
                            let is_foc = allow_keys && self.settings_focus == idx;
                            let (rect, resp) =
                                ui.allocate_exact_size(Vec2::new(card_w, 72.0), Sense::click());
                            if is_foc {
                                resp.scroll_to_me(Some(Align::Center));
                            }
                            let p = ui.painter();
                            p.rect_filled(rect, CornerRadius::same(16), preset.card_tint());
                            let st = if is_foc {
                                Stroke::new(2.4_f32, preset.primary())
                            } else if is_sel {
                                Stroke::new(1.8_f32, Color32::WHITE)
                            } else {
                                Stroke::new(
                                    1.0_f32,
                                    Color32::from_rgba_unmultiplied(255, 255, 255, 32),
                                )
                            };
                            p.rect_stroke(rect, CornerRadius::same(16), st, StrokeKind::Inside);

                            // Dual-tone color orbs
                            let c1 = Pos2::new(rect.left() + 22.0, rect.center().y);
                            let c2 = Pos2::new(rect.left() + 34.0, rect.center().y);
                            p.circle_filled(c1, 8.5, preset.primary());
                            p.circle_filled(c2, 8.5, preset.secondary());

                            p.text(
                                Pos2::new(rect.left() + 52.0, rect.top() + 25.0),
                                Align2::LEFT_CENTER,
                                preset.title(),
                                FontId::proportional(15.0),
                                Color32::WHITE,
                            );
                            p.text(
                                Pos2::new(rect.left() + 52.0, rect.top() + 47.0),
                                Align2::LEFT_CENTER,
                                if is_sel { "Active Theme" } else { "Select" },
                                FontId::proportional(11.5),
                                if is_sel {
                                    preset.primary()
                                } else {
                                    Color32::from_rgb(156, 170, 188)
                                },
                            );
                            if resp.clicked() {
                                chosen = Some(idx);
                            }
                            if col < 2 {
                                ui.add_space(12.0);
                            }
                        }
                    });
                    ui.add_space(10.0);
                }

                ui.add_space(10.0);

                // Settings Rows (indices 6..=16)
                let s = &self.state.settings;
                let rows: [(usize, &str, String); 11] = [
                    (6, "Display Typography", s.font_pairing.label().to_string()),
                    (7, "Glass Crystal Intensity", s.glass_intensity.label().to_string()),
                    (
                        8,
                        "4K Cinema Backdrop Blur",
                        if s.backdrop_blur_enabled { "ON" } else { "OFF" }.into(),
                    ),
                    (
                        9,
                        "Poster Aura Glow Shadow",
                        if s.card_shadow_glow { "ON" } else { "OFF" }.into(),
                    ),
                    (10, "Swipe Spring Speed", s.swipe_speed.label().to_string()),
                    (11, "TV Remote Control Scheme", s.control_scheme.label().to_string()),
                    (
                        12,
                        "Default Trailer Video Quality",
                        s.default_video_quality.label().to_string(),
                    ),
                    (13, "Cloud Sync Target", s.sync_target_mode.label().to_string()),
                    (
                        14,
                        "Hide Already Tracked Titles",
                        if s.hide_already_tracked { "ON" } else { "OFF" }.into(),
                    ),
                    (15, "Reset All Passed Titles", "Restore to Deck".into()),
                    (16, "Support Sway · Buy Me a Coffee", "Show 4K QR Code".into()),
                ];

                for (r_idx, title, val) in rows {
                    let is_foc = allow_keys && self.settings_focus == r_idx;
                    ui.horizontal(|ui| {
                        ui.add_space(48.0);
                        let w = ui.available_width() - 96.0;
                        let (rect, resp) =
                            ui.allocate_exact_size(Vec2::new(w, 44.0), Sense::click());
                        if is_foc {
                            resp.scroll_to_me(Some(Align::Center));
                        }
                        let p = ui.painter();
                        p.rect_filled(
                            rect,
                            CornerRadius::same(12),
                            if is_foc {
                                Color32::from_rgba_unmultiplied(
                                    primary.r(),
                                    primary.g(),
                                    primary.b(),
                                    48,
                                )
                            } else {
                                Color32::from_rgba_unmultiplied(255, 255, 255, 13)
                            },
                        );
                        p.rect_stroke(
                            rect,
                            CornerRadius::same(12),
                            if is_foc {
                                Stroke::new(2.0_f32, primary)
                            } else {
                                Stroke::new(
                                    1.0_f32,
                                    Color32::from_rgba_unmultiplied(255, 255, 255, 24),
                                )
                            },
                            StrokeKind::Inside,
                        );
                        p.text(
                            Pos2::new(rect.left() + 18.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            title,
                            FontId::proportional(14.5),
                            Color32::WHITE,
                        );
                        p.text(
                            Pos2::new(rect.right() - 18.0, rect.center().y),
                            Align2::RIGHT_CENTER,
                            &val,
                            FontId::proportional(14.0),
                            primary,
                        );
                        if resp.clicked() {
                            chosen = Some(r_idx);
                        }
                    });
                    ui.add_space(6.0);
                }
                ui.add_space(36.0);
            });

        if let Some(id) = chosen {
            match id {
                0..=5 => {
                    self.state.settings.theme_preset = ThemePreset::ALL[id];
                    self.persist();
                }
                6 => {
                    self.state.settings.font_pairing = self.state.settings.font_pairing.next();
                    self.persist();
                }
                7 => {
                    self.state.settings.glass_intensity =
                        self.state.settings.glass_intensity.next();
                    self.persist();
                }
                8 => {
                    self.state.settings.backdrop_blur_enabled =
                        !self.state.settings.backdrop_blur_enabled;
                    self.persist();
                }
                9 => {
                    self.state.settings.card_shadow_glow = !self.state.settings.card_shadow_glow;
                    self.persist();
                }
                10 => {
                    self.state.settings.swipe_speed = self.state.settings.swipe_speed.next();
                    self.persist();
                }
                11 => {
                    self.state.settings.control_scheme = self.state.settings.control_scheme.next();
                    self.persist();
                }
                12 => {
                    self.state.settings.default_video_quality =
                        self.state.settings.default_video_quality.next();
                    self.persist();
                }
                13 => {
                    self.state.settings.sync_target_mode =
                        self.state.settings.sync_target_mode.next();
                    self.persist();
                }
                14 => {
                    self.state.settings.hide_already_tracked =
                        !self.state.settings.hide_already_tracked;
                    self.filter_deck_in_place();
                    self.persist();
                }
                15 => {
                    self.state
                        .tracked
                        .retain(|_, v| v.action != SwipeAction::Skipped);
                    self.persist();
                    self.spawn_fetch_deck(ctx.clone(), true, 0);
                    self.show_toast("Cleared passed history & refreshed deck", None);
                }
                16 => {
                    self.modal = ModalState::BuyMeACoffee;
                }
                _ => {}
            }
        }
    }

    // ========================================================================
    // MODALS: FILTERS & SEARCH, DETAILS & 2160p TRAILER, DEVICE AUTH, COFFEE
    // ========================================================================

    fn draw_modals(&mut self, ctx: &egui::Context) {
        let modal_clone = self.modal.clone();
        match modal_clone {
            ModalState::None => {}

            ModalState::Filters => {
                let theme = self.state.settings.theme_preset;
                let primary = theme.primary();
                let mut reload = false;
                let mut close_now = false;

                ctx.input(|i| {
                    if i.key_pressed(Key::ArrowDown) {
                        self.filter_row = (self.filter_row + 1).min(4);
                    } else if i.key_pressed(Key::ArrowUp) {
                        self.filter_row = self.filter_row.saturating_sub(1);
                    } else if i.key_pressed(Key::ArrowRight) {
                        match self.filter_row {
                            0 => {
                                let pos = MediaType::ALL
                                    .iter()
                                    .position(|&m| m == self.media_type)
                                    .unwrap_or(0);
                                self.media_type = MediaType::ALL[(pos + 1) % MediaType::ALL.len()];
                                reload = true;
                            }
                            1 => {
                                let pos = FeedCategory::ALL
                                    .iter()
                                    .position(|&c| c == self.category)
                                    .unwrap_or(0);
                                self.category =
                                    FeedCategory::ALL[(pos + 1) % FeedCategory::ALL.len()];
                                reload = true;
                            }
                            2 => {
                                self.genre_idx = (self.genre_idx + 1) % SUPPORTED_GENRES.len();
                                reload = true;
                            }
                            _ => {}
                        }
                    } else if i.key_pressed(Key::ArrowLeft) {
                        match self.filter_row {
                            0 => {
                                let pos = MediaType::ALL
                                    .iter()
                                    .position(|&m| m == self.media_type)
                                    .unwrap_or(0);
                                self.media_type = MediaType::ALL
                                    [(pos + MediaType::ALL.len() - 1) % MediaType::ALL.len()];
                                reload = true;
                            }
                            1 => {
                                let pos = FeedCategory::ALL
                                    .iter()
                                    .position(|&c| c == self.category)
                                    .unwrap_or(0);
                                self.category = FeedCategory::ALL
                                    [(pos + FeedCategory::ALL.len() - 1) % FeedCategory::ALL.len()];
                                reload = true;
                            }
                            2 => {
                                self.genre_idx = (self.genre_idx + SUPPORTED_GENRES.len() - 1)
                                    % SUPPORTED_GENRES.len();
                                reload = true;
                            }
                            _ => {}
                        }
                    } else if (i.key_pressed(Key::Enter) || i.key_pressed(Key::Space))
                        && self.filter_row == 4
                    {
                        close_now = true;
                    }
                });

                self.modal_backdrop_window(ctx, "filters_modal", 560.0, |this, ui| {
                    ui.label(
                        RichText::new("Deck Filters & Search")
                            .font(this.display_font(22.0))
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.add_space(12.0);

                    // Row 0: Media Type
                    ui.label(
                        RichText::new(if this.filter_row == 0 {
                            "▸ Content Type"
                        } else {
                            "Content Type"
                        })
                        .font(FontId::proportional(12.5))
                        .color(primary),
                    );
                    ui.horizontal_wrapped(|ui| {
                        for mt in MediaType::ALL {
                            if Self::glass_pill_button(
                                ui,
                                mt.label(),
                                this.media_type == mt,
                                this.filter_row == 0 && this.media_type == mt,
                                primary,
                            ) {
                                this.media_type = mt;
                                reload = true;
                            }
                        }
                    });
                    ui.add_space(10.0);

                    // Row 1: Category
                    ui.label(
                        RichText::new(if this.filter_row == 1 {
                            "▸ Discovery Feed"
                        } else {
                            "Discovery Feed"
                        })
                        .font(FontId::proportional(12.5))
                        .color(primary),
                    );
                    ui.horizontal_wrapped(|ui| {
                        for cat in FeedCategory::ALL {
                            if Self::glass_pill_button(
                                ui,
                                cat.label(),
                                this.category == cat,
                                this.filter_row == 1 && this.category == cat,
                                primary,
                            ) {
                                this.category = cat;
                                reload = true;
                            }
                        }
                    });
                    ui.add_space(10.0);

                    // Row 2: Genre
                    ui.label(
                        RichText::new(if this.filter_row == 2 {
                            "▸ Genre"
                        } else {
                            "Genre"
                        })
                        .font(FontId::proportional(12.5))
                        .color(primary),
                    );
                    ui.horizontal_wrapped(|ui| {
                        for (gi, g) in SUPPORTED_GENRES.iter().enumerate() {
                            if Self::glass_pill_button(
                                ui,
                                g,
                                this.genre_idx == gi,
                                this.filter_row == 2 && this.genre_idx == gi,
                                primary,
                            ) {
                                this.genre_idx = gi;
                                reload = true;
                            }
                        }
                    });
                    ui.add_space(12.0);

                    // Row 3: Live Search
                    ui.label(
                        RichText::new(if this.filter_row == 3 {
                            "▸ Search Title"
                        } else {
                            "Search Title"
                        })
                        .font(FontId::proportional(12.5))
                        .color(primary),
                    );
                    let edit = egui::TextEdit::singleline(&mut this.search_query)
                        .hint_text("Search movies, series, or anime…")
                        .desired_width(f32::INFINITY);
                    let r = ui.add(edit);
                    if this.filter_row == 3 {
                        r.request_focus();
                    }
                    if r.changed() {
                        reload = true;
                    }
                    ui.add_space(14.0);

                    ui.horizontal(|ui| {
                        if Self::action_dock_pill(ui, "Apply & Return to Deck", primary, this.filter_row == 4) {
                            close_now = true;
                        }
                        if !this.search_query.is_empty() {
                            ui.add_space(8.0);
                            if Self::action_dock_pill(ui, "Clear Search", theme.pass_color(), false) {
                                this.search_query.clear();
                                reload = true;
                            }
                        }
                    });
                });

                if reload {
                    self.spawn_fetch_deck(ctx.clone(), true, 0);
                }
                if close_now {
                    self.modal = ModalState::None;
                }
            }

            ModalState::Details(item) => {
                let theme = self.state.settings.theme_preset;
                let primary = theme.primary();
                let cur_rating = self
                    .state
                    .tracked
                    .get(&item.id)
                    .and_then(|e| e.user_rating)
                    .unwrap_or(8);

                let mut trigger: Option<usize> = None;
                let mut rating_delta: i32 = 0;
                ctx.input(|i| {
                    if i.key_pressed(Key::ArrowRight) {
                        if self.detail_btn_idx == 4 {
                            rating_delta = 1;
                        } else {
                            self.detail_btn_idx = (self.detail_btn_idx + 1).min(5);
                        }
                    } else if i.key_pressed(Key::ArrowLeft) {
                        if self.detail_btn_idx == 4 {
                            rating_delta = -1;
                        } else {
                            self.detail_btn_idx = self.detail_btn_idx.saturating_sub(1);
                        }
                    } else if i.key_pressed(Key::ArrowDown) {
                        self.detail_btn_idx = 4;
                    } else if i.key_pressed(Key::ArrowUp) && self.detail_btn_idx >= 4 {
                        self.detail_btn_idx = 0;
                    } else if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                        trigger = Some(self.detail_btn_idx);
                    }
                });

                self.modal_backdrop_window(ctx, "details_modal", 660.0, |this, ui| {
                    ui.horizontal(|ui| {
                        // Poster preview
                        let (pr, _) = ui.allocate_exact_size(Vec2::new(132.0, 198.0), Sense::hover());
                        if let Some(tex) =
                            this.images
                                .get_or_request(&item.poster_url, ImgKind::Poster, ctx)
                        {
                            ui.painter().image(
                                tex.id(),
                                pr,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            ui.painter()
                                .rect_filled(pr, CornerRadius::same(12), theme.card_tint());
                        }
                        ui.painter().rect_stroke(
                            pr,
                            CornerRadius::same(12),
                            Stroke::new(1.2_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 50)),
                            StrokeKind::Inside,
                        );

                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(&item.title)
                                    .font(this.display_font(25.0))
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} · ★ {}",
                                    item.media_type.label(),
                                    item.year,
                                    if item.imdb_rating.is_empty() {
                                        "N/A"
                                    } else {
                                        &item.imdb_rating
                                    }
                                ))
                                .font(FontId::proportional(13.0))
                                .color(theme.secondary()),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(&item.overview)
                                    .font(FontId::proportional(13.5))
                                    .color(Color32::from_rgb(212, 222, 235)),
                            );
                            if !item.cast.is_empty() {
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(format!("Cast: {}", item.cast.join(", ")))
                                        .font(FontId::proportional(12.0))
                                        .color(Color32::from_rgb(156, 170, 188)),
                                );
                            }
                        });
                    });

                    ui.add_space(16.0);
                    ui.horizontal_wrapped(|ui| {
                        let q_lbl = this.state.settings.default_video_quality.label();
                        if Self::action_dock_pill(
                            ui,
                            &format!("▶ Watch {q_lbl} Trailer"),
                            primary,
                            this.detail_btn_idx == 0,
                        ) {
                            trigger = Some(0);
                        }
                        ui.add_space(6.0);
                        if Self::action_dock_pill(
                            ui,
                            "Watchlist",
                            theme.to_watch_color(),
                            this.detail_btn_idx == 1,
                        ) {
                            trigger = Some(1);
                        }
                        ui.add_space(6.0);
                        if Self::action_dock_pill(
                            ui,
                            "Watched ✓",
                            theme.watched_color(),
                            this.detail_btn_idx == 2,
                        ) {
                            trigger = Some(2);
                        }
                        ui.add_space(6.0);
                        if Self::action_dock_pill(
                            ui,
                            "Pass",
                            theme.pass_color(),
                            this.detail_btn_idx == 3,
                        ) {
                            trigger = Some(3);
                        }
                        ui.add_space(6.0);
                        if Self::action_dock_pill(
                            ui,
                            "Close",
                            Color32::from_rgb(160, 174, 192),
                            this.detail_btn_idx == 5,
                        ) {
                            trigger = Some(5);
                        }
                    });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let rate_lbl = format!(
                            "Your Rating: ★ {cur_rating}/10  (← / → adjust, OK save & sync)"
                        );
                        if Self::action_dock_pill(
                            ui,
                            &rate_lbl,
                            Color32::from_rgb(245, 196, 81),
                            this.detail_btn_idx == 4,
                        ) {
                            trigger = Some(4);
                        }
                    });
                });

                if rating_delta != 0 {
                    let next_r = (cur_rating + rating_delta).clamp(1, 10);
                    let entry = self
                        .state
                        .tracked
                        .entry(item.id.clone())
                        .or_insert_with(|| TrackedEntry {
                            item: item.clone(),
                            action: SwipeAction::Watched,
                            updated_at_ms: Utc::now().timestamp_millis(),
                            user_rating: Some(next_r),
                            synced_trakt: false,
                            synced_simkl: false,
                        });
                    entry.user_rating = Some(next_r);
                    self.persist();
                }

                if let Some(t_id) = trigger {
                    match t_id {
                        0 => self.start_trailer_playback(ctx, &item),
                        1 => {
                            self.modal = ModalState::None;
                            self.apply_swipe(ctx, item, SwipeAction::ToWatch);
                        }
                        2 => {
                            self.modal = ModalState::None;
                            self.apply_swipe(ctx, item, SwipeAction::Watched);
                        }
                        3 => {
                            self.modal = ModalState::None;
                            self.apply_swipe(ctx, item, SwipeAction::Skipped);
                        }
                        4 => {
                            let r = self
                                .state
                                .tracked
                                .get(&item.id)
                                .and_then(|e| e.user_rating)
                                .unwrap_or(8);
                            self.enqueue_sync_op(&item, SyncOpType::Rate, Some(r), ctx);
                            self.persist();
                            self.show_toast(
                                format!("Saved ★ {r}/10 rating for \"{}\"", item.title),
                                Some(SwipeAction::Watched),
                            );
                        }
                        _ => {
                            self.modal = ModalState::None;
                        }
                    }
                }
            }

            ModalState::DeviceAuth(sess) => {
                let primary = self.state.settings.theme_preset.primary();
                let qr = self.qr_texture(ctx, &sess.verification_url);
                let mut cancel = false;
                ctx.input(|i| {
                    if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                        cancel = true;
                    }
                });

                self.modal_backdrop_window(ctx, "device_auth_modal", 460.0, |this, ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(format!("Authorize {}", sess.provider.name()))
                                .font(this.display_font(24.0))
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "Scan the QR code or visit {} and enter:",
                                sess.verification_url
                            ))
                            .font(FontId::proportional(13.0))
                            .color(Color32::from_rgb(180, 194, 212)),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(&sess.user_code)
                                .font(this.display_font(34.0))
                                .strong()
                                .color(primary),
                        );
                        ui.add_space(12.0);
                        if let Some(tex) = qr {
                            let (qr_r, _) =
                                ui.allocate_exact_size(Vec2::splat(154.0), Sense::hover());
                            ui.painter().rect_filled(
                                qr_r.expand(6.0),
                                CornerRadius::same(12),
                                Color32::WHITE,
                            );
                            ui.painter().image(
                                tex.id(),
                                qr_r,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new("Waiting automatically for confirmation…")
                                .font(FontId::proportional(12.0))
                                .color(Color32::from_rgb(148, 163, 184)),
                        );
                        ui.add_space(10.0);
                        if Self::action_dock_pill(ui, "Cancel", primary, true) {
                            cancel = true;
                        }
                    });
                });

                if cancel {
                    self.modal = ModalState::None;
                }
            }

            ModalState::BuyMeACoffee => {
                let primary = self.state.settings.theme_preset.primary();
                let qr = self.qr_texture(ctx, "https://buymeacoffee.com/burbsklopek");
                let mut close = false;
                ctx.input(|i| {
                    if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                        close = true;
                    }
                });

                self.modal_backdrop_window(ctx, "coffee_modal", 420.0, |this, ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("Support Sway")
                                .font(this.display_font(24.0))
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Scan with your phone to visit buymeacoffee.com/burbsklopek")
                                .font(FontId::proportional(13.0))
                                .color(Color32::from_rgb(185, 198, 214)),
                        );
                        ui.add_space(14.0);
                        if let Some(tex) = qr {
                            let (qr_r, _) =
                                ui.allocate_exact_size(Vec2::splat(164.0), Sense::hover());
                            ui.painter().rect_filled(
                                qr_r.expand(6.0),
                                CornerRadius::same(14),
                                Color32::WHITE,
                            );
                            ui.painter().image(
                                tex.id(),
                                qr_r,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                        ui.add_space(14.0);
                        if Self::action_dock_pill(ui, "Done", primary, true) {
                            close = true;
                        }
                    });
                });

                if close {
                    self.modal = ModalState::None;
                }
            }
        }
    }

    fn modal_backdrop_window<F>(&mut self, ctx: &egui::Context, id: &str, width: f32, body: F)
    where
        F: FnOnce(&mut Self, &mut Ui),
    {
        let screen = ctx.screen_rect();
        let dim_painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("modal_scrim"),
        ));
        dim_painter.rect_filled(
            screen,
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(3, 6, 10, 188),
        );

        let theme = self.state.settings.theme_preset;
        let glass = self.state.settings.glass_intensity;
        let tint = theme.card_tint();
        let fill = Color32::from_rgba_unmultiplied(
            tint.r(),
            tint.g(),
            tint.b(),
            glass.fill_alpha().max(222),
        );

        egui::Area::new(egui::Id::new(id))
            .order(egui::Order::Tooltip)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(fill)
                    .stroke(Stroke::new(
                        1.5_f32,
                        Color32::from_rgba_unmultiplied(
                            theme.primary().r(),
                            theme.primary().g(),
                            theme.primary().b(),
                            glass.border_alpha().max(120),
                        ),
                    ))
                    .corner_radius(CornerRadius::same(24))
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        body(self, ui);
                    });
            });
    }

    fn draw_toast_overlay(&mut self, ctx: &egui::Context) {
        let Some(ref toast) = self.toast else { return };
        let elapsed = toast.shown_at.elapsed().as_secs_f32();
        if elapsed > 2.6 {
            self.toast = None;
            return;
        }
        ctx.request_repaint();
        let alpha_f = if elapsed > 2.1 {
            ((2.6 - elapsed) / 0.5).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let theme = self.state.settings.theme_preset;
        let dot_c = match toast.accent {
            Some(SwipeAction::Watched) => theme.watched_color(),
            Some(SwipeAction::ToWatch) => theme.to_watch_color(),
            Some(SwipeAction::Skipped) => theme.pass_color(),
            None => theme.primary(),
        };
        let a = (alpha_f * 238.0) as u8;

        egui::Area::new(egui::Id::new("sway_toast_pill"))
            .order(egui::Order::Tooltip)
            .anchor(Align2::LEFT_BOTTOM, Vec2::new(48.0, -22.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(Color32::from_rgba_unmultiplied(8, 12, 16, a))
                    .stroke(Stroke::new(
                        1.2_f32,
                        Color32::from_rgba_unmultiplied(dot_c.r(), dot_c.g(), dot_c.b(), a),
                    ))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dr, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                            ui.painter().circle_filled(dr.center(), 3.6, dot_c);
                            ui.label(
                                RichText::new(&toast.text)
                                    .font(FontId::proportional(13.0))
                                    .strong()
                                    .color(Color32::from_rgba_unmultiplied(248, 250, 252, a)),
                            );
                        });
                    });
            });
    }
}

impl eframe::App for SwayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.state.settings.theme_preset.bg_deep();
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.images.poll(ctx);
        self.drain_events(ctx);
        self.poll_active_device_auth(ctx);
        self.handle_back_and_menu(ctx);

        let screen = ctx.screen_rect();
        self.draw_backdrop_stage(ctx, screen);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                self.draw_top_rail(ui, ctx);
                match self.active_tab {
                    NavTab::Deck => self.draw_deck_screen(ui, ctx),
                    NavTab::Collection => self.draw_collection_screen(ui, ctx),
                    NavTab::SyncHub => self.draw_sync_hub_screen(ui, ctx),
                    NavTab::Themes => self.draw_themes_screen(ui, ctx),
                }
            });

        self.draw_modals(ctx);
        self.draw_toast_overlay(ctx);
    }
}
