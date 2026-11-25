# Sway Rust

[![Buy Me A Coffee](https://img.shields.io/badge/Buy%20Me%20a%20Coffee-Support-FFDD00?style=for-the-badge&logo=buy-me-a-coffee&logoColor=black)](https://buymeacoffee.com/burbsklopek)

A **Pure Native Rust 4K OpenGL ES 3.0 Swipe-Deck Watch Tracker** for **Android TV** (`com.burbsklopek.swayrust`), pairing a zero-GC 60/120fps 3D perspective poster deck with bidirectional **Trakt** and **Simkl** cloud sync.

## Highlights

- **Pure Native Rust Engine (`eframe` / `egui` / `glow` + `NativeActivity`)**:
  - Zero WebView, zero Jetpack Compose overhead, instant `<150ms` cold boot, deterministic 60/120fps frame pacing.
  - Custom 4-vertex 3D perspective quad projection (`egui::Mesh`) during card swipes and rewinds, paired with weightless resting levitation and orbital liquid-glass rim shimmer.
- **TV Remote Swipe Presets**:
  - **Three-Way Remote (Default)**: `→` **Watchlist** · `↑` **Watched ✓** · `←` **Pass** · `↓` **Undo Rewind** · `OK` **Details & Trailer** · `Back` **Focus Top Rail**.
  - **Split Left & Right**: `→` **Watched** · `←` **Watchlist** · `↓` **Pass**.
  - **Focus Buttons**: Standard D-Pad navigation across interactive action pills.
- **Native Trakt & Simkl Cloud Sync**:
  - **Trakt API v2**: OAuth2 Device Code + 4K QR activation (`trakt.tv/activate`), bidirectional `/sync/watchlist`, `/sync/history`, and `1–10 ★` user ratings.
  - **Simkl API (Auth V2 RFC 8628 + V1 PIN Fallback)**: Device PIN + 4K QR activation (`simkl.com/pin`), bidirectional sync across TV Shows, Anime, and Movies.
  - **LAN Phone Companion (`http://<tv-ip>:8766`)**: Scan the on-screen QR code with your phone to paste Trakt or Simkl API credentials in seconds (automatically skips VPN tunnel interfaces so home Wi-Fi phones connect directly).
- **Zero-Key Instant Discovery**:
  - Works out of the box via Cinemeta (`IMDb` high-res posters, fanart backdrops, cast, awards, trailers) and Kitsu Anime, with automatic 5-card lookahead GPU texture prefetching.
- **Liquid-Glass & 4K Customization**:
  - **6 Dual-Tone Glass Themes**: `Aurora Prism`, `Midnight Nebula`, `Crimson Velvet`, `Obsidian Gold`, `Arctic Sapphire`, `Titanium Neon`.
  - **4 Bundled Cinema Display Fonts**: `Outfit`, `Fraunces`, `Syne`, and `Space Grotesk` paired with `Plus Jakarta Sans`.
  - Separable multi-pass cinema backdrop blur, adjustable glass crystal intensity, and Ultra HD trailer playback.

## Build

```bash
chmod +x scripts/build-apk.sh
./scripts/build-apk.sh
```
