use egui::Color32;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaType {
    Show,
    Anime,
    Movie,
}

impl MediaType {
    pub const ALL: [MediaType; 3] = [MediaType::Show, MediaType::Anime, MediaType::Movie];

    pub fn label(self) -> &'static str {
        match self {
            MediaType::Show => "TV Shows",
            MediaType::Anime => "Anime",
            MediaType::Movie => "Movies",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeedCategory {
    Trending,
    TopRated,
    NewReleases,
    ForYou,
}

impl FeedCategory {
    pub const ALL: [FeedCategory; 4] = [
        FeedCategory::Trending,
        FeedCategory::TopRated,
        FeedCategory::NewReleases,
        FeedCategory::ForYou,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FeedCategory::Trending => "Trending",
            FeedCategory::TopRated => "Top Rated",
            FeedCategory::NewReleases => "Fresh & New",
            FeedCategory::ForYou => "For You",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwipeAction {
    ToWatch,
    Watched,
    Skipped,
}

impl SwipeAction {
    pub fn label(self) -> &'static str {
        match self {
            SwipeAction::ToWatch => "Watchlist",
            SwipeAction::Watched => "Watched",
            SwipeAction::Skipped => "Passed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncProvider {
    Trakt,
    Simkl,
}

impl SyncProvider {
    pub fn name(self) -> &'static str {
        match self {
            SyncProvider::Trakt => "Trakt",
            SyncProvider::Simkl => "Simkl",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncOpType {
    AddWatched,
    AddWatchlist,
    RemoveWatched,
    RemoveWatchlist,
    Rate,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalIds {
    pub imdb: Option<String>,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub trakt: Option<i64>,
    pub simkl: Option<i64>,
    pub mal: Option<i64>,
    pub slug: Option<String>,
}

impl ExternalIds {
    pub fn canonical_key(&self, fallback_title: &str, year: &str) -> String {
        if let Some(v) = self.imdb.as_deref().filter(|s| !s.trim().is_empty()) {
            return format!("imdb:{v}");
        }
        if let Some(v) = self.simkl.filter(|&n| n > 0) {
            return format!("simkl:{v}");
        }
        if let Some(v) = self.trakt.filter(|&n| n > 0) {
            return format!("trakt:{v}");
        }
        if let Some(v) = self.tmdb.filter(|&n| n > 0) {
            return format!("tmdb:{v}");
        }
        if let Some(v) = self.tvdb.filter(|&n| n > 0) {
            return format!("tvdb:{v}");
        }
        if let Some(v) = self.mal.filter(|&n| n > 0) {
            return format!("mal:{v}");
        }
        if let Some(v) = self.slug.as_deref().filter(|s| !s.trim().is_empty()) {
            return format!("slug:{v}");
        }
        let norm: String = fallback_title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        format!("title:{norm}-{year}")
    }

    pub fn merge_with(&self, other: &ExternalIds) -> ExternalIds {
        ExternalIds {
            imdb: self.imdb.clone().or_else(|| other.imdb.clone()),
            tmdb: self.tmdb.or(other.tmdb),
            tvdb: self.tvdb.or(other.tvdb),
            trakt: self.trakt.or(other.trakt),
            simkl: self.simkl.or(other.simkl),
            mal: self.mal.or(other.mal),
            slug: self.slug.clone().or_else(|| other.slug.clone()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: String,
    pub media_type: MediaType,
    pub title: String,
    pub year: String,
    pub status: String,
    pub imdb_rating: String,
    pub runtime: String,
    pub genres: Vec<String>,
    pub overview: String,
    pub cast: Vec<String>,
    pub creators: Vec<String>,
    pub awards: Option<String>,
    pub network: Option<String>,
    pub seasons_count: Option<u32>,
    pub poster_url: String,
    pub backdrop_url: String,
    pub logo_url: Option<String>,
    pub trailer_youtube_id: Option<String>,
    pub ids: ExternalIds,
    pub source_badge: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedEntry {
    pub item: MediaItem,
    pub action: SwipeAction,
    pub updated_at_ms: i64,
    pub user_rating: Option<i32>,
    pub synced_trakt: bool,
    pub synced_simkl: bool,
}

#[derive(Debug, Clone)]
pub struct SwipeHistoryStep {
    pub item: MediaItem,
    pub previous_entry: Option<TrackedEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingSyncOp {
    pub op_id: String,
    pub provider: SyncProvider,
    pub op_type: SyncOpType,
    pub item: MediaItem,
    pub rating: Option<i32>,
    pub attempts: u32,
}

#[derive(Debug, Clone)]
pub struct DeviceAuthSession {
    pub provider: SyncProvider,
    pub user_code: String,
    pub device_code: String,
    pub verification_url: String,
    pub expires_in_sec: u64,
    pub interval_sec: u64,
    pub is_legacy_pin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemePreset {
    AuroraPrism,
    MidnightNebula,
    CrimsonVelvet,
    ObsidianGold,
    ArcticSapphire,
    TitaniumNeon,
}

impl ThemePreset {
    pub const ALL: [ThemePreset; 6] = [
        ThemePreset::AuroraPrism,
        ThemePreset::MidnightNebula,
        ThemePreset::CrimsonVelvet,
        ThemePreset::ObsidianGold,
        ThemePreset::ArcticSapphire,
        ThemePreset::TitaniumNeon,
    ];

    pub fn title(self) -> &'static str {
        match self {
            ThemePreset::AuroraPrism => "Aurora Prism",
            ThemePreset::MidnightNebula => "Midnight Nebula",
            ThemePreset::CrimsonVelvet => "Crimson Velvet",
            ThemePreset::ObsidianGold => "Obsidian Gold",
            ThemePreset::ArcticSapphire => "Arctic Sapphire",
            ThemePreset::TitaniumNeon => "Titanium Neon",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            ThemePreset::AuroraPrism => "Nocturne emerald-slate glass with prism teal & champagne gold",
            ThemePreset::MidnightNebula => "Obsidian cosmos with electric violet & icy cyan glass",
            ThemePreset::CrimsonVelvet => "Dark screening-room ruby with warm coral & amber light",
            ThemePreset::ObsidianGold => "Pure 4K OLED blackout with warm champagne & bronze",
            ThemePreset::ArcticSapphire => "Deep glacial navy with crisp sky blue & periwinkle",
            ThemePreset::TitaniumNeon => "Smoked dark titanium with magenta & cyan refraction",
        }
    }

    pub fn bg_deep(self) -> Color32 {
        match self {
            ThemePreset::AuroraPrism => Color32::from_rgb(5, 11, 12),
            ThemePreset::MidnightNebula => Color32::from_rgb(7, 7, 17),
            ThemePreset::CrimsonVelvet => Color32::from_rgb(12, 5, 8),
            ThemePreset::ObsidianGold => Color32::from_rgb(4, 4, 5),
            ThemePreset::ArcticSapphire => Color32::from_rgb(4, 10, 20),
            ThemePreset::TitaniumNeon => Color32::from_rgb(8, 9, 16),
        }
    }

    pub fn bg_glow(self) -> Color32 {
        match self {
            ThemePreset::AuroraPrism => Color32::from_rgb(18, 53, 50),
            ThemePreset::MidnightNebula => Color32::from_rgb(37, 24, 78),
            ThemePreset::CrimsonVelvet => Color32::from_rgb(66, 16, 36),
            ThemePreset::ObsidianGold => Color32::from_rgb(43, 34, 18),
            ThemePreset::ArcticSapphire => Color32::from_rgb(18, 48, 84),
            ThemePreset::TitaniumNeon => Color32::from_rgb(45, 25, 66),
        }
    }

    pub fn card_tint(self) -> Color32 {
        match self {
            ThemePreset::AuroraPrism => Color32::from_rgb(13, 25, 27),
            ThemePreset::MidnightNebula => Color32::from_rgb(18, 17, 38),
            ThemePreset::CrimsonVelvet => Color32::from_rgb(26, 11, 19),
            ThemePreset::ObsidianGold => Color32::from_rgb(19, 17, 14),
            ThemePreset::ArcticSapphire => Color32::from_rgb(11, 25, 46),
            ThemePreset::TitaniumNeon => Color32::from_rgb(21, 23, 38),
        }
    }

    pub fn primary(self) -> Color32 {
        match self {
            ThemePreset::AuroraPrism => Color32::from_rgb(45, 212, 191),
            ThemePreset::MidnightNebula => Color32::from_rgb(168, 85, 247),
            ThemePreset::CrimsonVelvet => Color32::from_rgb(244, 63, 94),
            ThemePreset::ObsidianGold => Color32::from_rgb(245, 196, 81),
            ThemePreset::ArcticSapphire => Color32::from_rgb(56, 189, 248),
            ThemePreset::TitaniumNeon => Color32::from_rgb(236, 72, 153),
        }
    }

    pub fn secondary(self) -> Color32 {
        match self {
            ThemePreset::AuroraPrism => Color32::from_rgb(245, 196, 81),
            ThemePreset::MidnightNebula => Color32::from_rgb(56, 189, 248),
            ThemePreset::CrimsonVelvet => Color32::from_rgb(251, 191, 36),
            ThemePreset::ObsidianGold => Color32::from_rgb(253, 230, 138),
            ThemePreset::ArcticSapphire => Color32::from_rgb(129, 140, 248),
            ThemePreset::TitaniumNeon => Color32::from_rgb(34, 211, 238),
        }
    }

    pub fn watched_color(self) -> Color32 {
        match self {
            ThemePreset::AuroraPrism => Color32::from_rgb(245, 196, 81),
            ThemePreset::MidnightNebula => Color32::from_rgb(245, 196, 81),
            ThemePreset::CrimsonVelvet => Color32::from_rgb(52, 211, 153),
            ThemePreset::ObsidianGold => Color32::from_rgb(245, 196, 81),
            ThemePreset::ArcticSapphire => Color32::from_rgb(74, 222, 128),
            ThemePreset::TitaniumNeon => Color32::from_rgb(232, 121, 249),
        }
    }

    pub fn to_watch_color(self) -> Color32 {
        match self {
            ThemePreset::AuroraPrism => Color32::from_rgb(45, 212, 191),
            ThemePreset::MidnightNebula => Color32::from_rgb(56, 189, 248),
            ThemePreset::CrimsonVelvet => Color32::from_rgb(251, 191, 36),
            ThemePreset::ObsidianGold => Color32::from_rgb(56, 189, 248),
            ThemePreset::ArcticSapphire => Color32::from_rgb(56, 189, 248),
            ThemePreset::TitaniumNeon => Color32::from_rgb(34, 211, 238),
        }
    }

    pub fn pass_color(self) -> Color32 {
        match self {
            ThemePreset::AuroraPrism => Color32::from_rgb(244, 63, 94),
            ThemePreset::MidnightNebula => Color32::from_rgb(251, 113, 133),
            ThemePreset::CrimsonVelvet => Color32::from_rgb(244, 63, 94),
            ThemePreset::ObsidianGold => Color32::from_rgb(239, 68, 68),
            ThemePreset::ArcticSapphire => Color32::from_rgb(251, 113, 133),
            ThemePreset::TitaniumNeon => Color32::from_rgb(244, 63, 94),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlassIntensity {
    Subtle,
    Frosted,
    Crystal,
}

impl GlassIntensity {
    pub fn next(self) -> Self {
        match self {
            GlassIntensity::Subtle => GlassIntensity::Frosted,
            GlassIntensity::Frosted => GlassIntensity::Crystal,
            GlassIntensity::Crystal => GlassIntensity::Subtle,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            GlassIntensity::Subtle => "Smoked",
            GlassIntensity::Frosted => "Frosted",
            GlassIntensity::Crystal => "Crystal",
        }
    }
    pub fn fill_alpha(self) -> u8 {
        match self {
            GlassIntensity::Subtle => 165,
            GlassIntensity::Frosted => 198,
            GlassIntensity::Crystal => 224,
        }
    }
    pub fn border_alpha(self) -> u8 {
        match self {
            GlassIntensity::Subtle => 38,
            GlassIntensity::Frosted => 64,
            GlassIntensity::Crystal => 96,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwipeSpeed {
    Snappy,
    Fluid,
    Measured,
}

impl SwipeSpeed {
    pub fn next(self) -> Self {
        match self {
            SwipeSpeed::Snappy => SwipeSpeed::Fluid,
            SwipeSpeed::Fluid => SwipeSpeed::Measured,
            SwipeSpeed::Measured => SwipeSpeed::Snappy,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SwipeSpeed::Snappy => "Snappy",
            SwipeSpeed::Fluid => "Fluid",
            SwipeSpeed::Measured => "Measured",
        }
    }
    pub fn duration_sec(self) -> f32 {
        match self {
            SwipeSpeed::Snappy => 0.19,
            SwipeSpeed::Fluid => 0.27,
            SwipeSpeed::Measured => 0.36,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwipeControlScheme {
    ThreeWayRemote,
    SplitLeftRight,
    ButtonDock,
}

impl SwipeControlScheme {
    pub fn next(self) -> Self {
        match self {
            SwipeControlScheme::ThreeWayRemote => SwipeControlScheme::SplitLeftRight,
            SwipeControlScheme::SplitLeftRight => SwipeControlScheme::ButtonDock,
            SwipeControlScheme::ButtonDock => SwipeControlScheme::ThreeWayRemote,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SwipeControlScheme::ThreeWayRemote => "Three-way remote",
            SwipeControlScheme::SplitLeftRight => "Split left & right",
            SwipeControlScheme::ButtonDock => "Focus buttons",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            SwipeControlScheme::ThreeWayRemote => {
                "Right: Watchlist · Up: Watched · Left: Pass · Down: Undo · OK: Details & 4K Trailer"
            }
            SwipeControlScheme::SplitLeftRight => {
                "Right: Watched · Left: Watchlist · Down: Pass · OK: Details & 4K Trailer"
            }
            SwipeControlScheme::ButtonDock => {
                "Standard D-Pad navigation across the action pills below the synopsis"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoQuality {
    Uhd2160p,
    Qhd1440p,
    Fhd1080p,
    Auto,
}

impl VideoQuality {
    pub fn next(self) -> Self {
        match self {
            VideoQuality::Uhd2160p => VideoQuality::Qhd1440p,
            VideoQuality::Qhd1440p => VideoQuality::Fhd1080p,
            VideoQuality::Fhd1080p => VideoQuality::Auto,
            VideoQuality::Auto => VideoQuality::Uhd2160p,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            VideoQuality::Uhd2160p => "2160p 4K",
            VideoQuality::Qhd1440p => "1440p",
            VideoQuality::Fhd1080p => "1080p",
            VideoQuality::Auto => "Auto (2160p)",
        }
    }
    pub fn height_px(self) -> i32 {
        match self {
            VideoQuality::Uhd2160p | VideoQuality::Auto => 2160,
            VideoQuality::Qhd1440p => 1440,
            VideoQuality::Fhd1080p => 1080,
        }
    }
    pub fn youtube_vq(self) -> &'static str {
        match self {
            VideoQuality::Uhd2160p | VideoQuality::Auto => "hd2160",
            VideoQuality::Qhd1440p => "hd1440",
            VideoQuality::Fhd1080p => "hd1080",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncTargetMode {
    BothConnected,
    TraktOnly,
    SimklOnly,
}

impl SyncTargetMode {
    pub fn next(self) -> Self {
        match self {
            SyncTargetMode::BothConnected => SyncTargetMode::TraktOnly,
            SyncTargetMode::TraktOnly => SyncTargetMode::SimklOnly,
            SyncTargetMode::SimklOnly => SyncTargetMode::BothConnected,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SyncTargetMode::BothConnected => "Both services",
            SyncTargetMode::TraktOnly => "Trakt only",
            SyncTargetMode::SimklOnly => "Simkl only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontPairing {
    OutfitDisplay,
    FrauncesSerif,
    SyneWide,
    SpaceGrotesk,
}

impl FontPairing {
    pub const ALL: [FontPairing; 4] = [
        FontPairing::OutfitDisplay,
        FontPairing::FrauncesSerif,
        FontPairing::SyneWide,
        FontPairing::SpaceGrotesk,
    ];

    pub fn next(self) -> Self {
        match self {
            FontPairing::OutfitDisplay => FontPairing::FrauncesSerif,
            FontPairing::FrauncesSerif => FontPairing::SyneWide,
            FontPairing::SyneWide => FontPairing::SpaceGrotesk,
            FontPairing::SpaceGrotesk => FontPairing::OutfitDisplay,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FontPairing::OutfitDisplay => "Outfit Display",
            FontPairing::FrauncesSerif => "Fraunces Serif",
            FontPairing::SyneWide => "Syne Wide",
            FontPairing::SpaceGrotesk => "Space Grotesk",
        }
    }

    pub fn family_key(self) -> &'static str {
        match self {
            FontPairing::OutfitDisplay => "outfit",
            FontPairing::FrauncesSerif => "fraunces",
            FontPairing::SyneWide => "syne",
            FontPairing::SpaceGrotesk => "space_grotesk",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme_preset: ThemePreset,
    pub font_pairing: FontPairing,
    pub glass_intensity: GlassIntensity,
    pub backdrop_blur_enabled: bool,
    pub card_shadow_glow: bool,
    pub swipe_speed: SwipeSpeed,
    pub control_scheme: SwipeControlScheme,
    pub default_video_quality: VideoQuality,
    pub sync_target_mode: SyncTargetMode,
    pub hide_already_tracked: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme_preset: ThemePreset::AuroraPrism,
            font_pairing: FontPairing::OutfitDisplay,
            glass_intensity: GlassIntensity::Frosted,
            backdrop_blur_enabled: true,
            card_shadow_glow: true,
            swipe_speed: SwipeSpeed::Snappy,
            control_scheme: SwipeControlScheme::ThreeWayRemote,
            default_video_quality: VideoQuality::Uhd2160p,
            sync_target_mode: SyncTargetMode::BothConnected,
            hide_already_tracked: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub trakt_client_id: String,
    pub trakt_client_secret: String,
    pub trakt_access_token: String,
    pub trakt_refresh_token: String,
    pub trakt_username: String,
    pub trakt_last_sync_ms: i64,

    pub simkl_client_id: String,
    pub simkl_access_token: String,
    pub simkl_refresh_token: String,
    pub simkl_username: String,
    pub simkl_last_sync_ms: i64,

    pub tmdb_api_key: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedState {
    pub settings: AppSettings,
    pub credentials: StoredCredentials,
    pub tracked: HashMap<String, TrackedEntry>,
    pub pending_ops: Vec<PendingSyncOp>,
}
