use crate::bridge::url_encode;
use crate::models::{ExternalIds, FeedCategory, MediaItem, MediaType};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

const CINEMETA_BASE: &str = "https://v3-cinemeta.strem.io";
const KITSU_BASE: &str = "https://anime-kitsu.strem.fun";
const USER_AGENT: &str = "SwayRust/1.0.0 (Android TV 12; 4K)";

pub const SUPPORTED_GENRES: [&str; 12] = [
    "All",
    "Sci-Fi",
    "Drama",
    "Thriller",
    "Crime",
    "Comedy",
    "Fantasy",
    "Mystery",
    "Action",
    "Horror",
    "Animation",
    "Documentary",
];

pub fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(15))
        .timeout_write(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build()
}

pub fn metahub_poster(imdb: Option<&str>) -> String {
    match imdb.filter(|s| !s.trim().is_empty()) {
        Some(id) => format!("https://images.metahub.space/poster/large/{id}/img"),
        None => String::new(),
    }
}

pub fn metahub_backdrop(imdb: Option<&str>) -> String {
    match imdb.filter(|s| !s.trim().is_empty()) {
        Some(id) => format!("https://images.metahub.space/background/large/{id}/img"),
        None => String::new(),
    }
}

pub fn metahub_logo(imdb: Option<&str>) -> Option<String> {
    imdb.filter(|s| !s.trim().is_empty())
        .map(|id| format!("https://images.metahub.space/logo/medium/{id}/img"))
}

pub fn is_valid_youtube_id(id: &str) -> bool {
    let t = id.trim();
    t.len() == 11
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn fetch_catalog_page(
    media_type: MediaType,
    category: FeedCategory,
    genre: &str,
    page_index: usize,
) -> Vec<MediaItem> {
    let online = match media_type {
        MediaType::Anime => fetch_anime_catalog(category, page_index),
        MediaType::Show | MediaType::Movie => {
            fetch_cinemeta_catalog(media_type, category, genre, page_index)
        }
    };
    if !online.is_empty() {
        return online;
    }
    if page_index == 0 {
        let seeds = fallback_seed_deck(media_type);
        if genre == "All" || genre.trim().is_empty() {
            seeds
        } else {
            let filtered: Vec<MediaItem> = seeds
                .iter()
                .filter(|s| s.genres.iter().any(|g| g.eq_ignore_ascii_case(genre)))
                .cloned()
                .collect();
            if filtered.is_empty() {
                seeds
            } else {
                filtered
            }
        }
    } else {
        Vec::new()
    }
}

pub fn search_catalog(query: &str, media_type: MediaType) -> Vec<MediaItem> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let enc = url_encode(trimmed);
    let agent = http_agent();

    if media_type == MediaType::Anime {
        let kitsu = fetch_stremio_metas(
            &agent,
            &format!("{KITSU_BASE}/catalog/anime/kitsu-anime-list/search={enc}.json"),
            MediaType::Anime,
            "Anime · 4K",
        );
        if !kitsu.is_empty() {
            return kitsu;
        }
        return fetch_stremio_metas(
            &agent,
            &format!("{CINEMETA_BASE}/catalog/series/top/search={enc}.json"),
            MediaType::Anime,
            "Anime · 4K",
        );
    }

    let seg = if media_type == MediaType::Movie {
        "movie"
    } else {
        "series"
    };
    fetch_stremio_metas(
        &agent,
        &format!("{CINEMETA_BASE}/catalog/{seg}/top/search={enc}.json"),
        media_type,
        "IMDb · 4K",
    )
}

pub fn enrich_item_details(item: &MediaItem) -> Option<MediaItem> {
    let imdb = item.ids.imdb.as_deref().filter(|s| s.starts_with("tt"))?;
    let agent = http_agent();
    let seg = if item.media_type == MediaType::Movie {
        "movie"
    } else {
        "series"
    };
    fetch_cinemeta_meta(&agent, item, imdb, seg).or_else(|| {
        if item.media_type == MediaType::Anime {
            fetch_cinemeta_meta(&agent, item, imdb, "movie")
        } else {
            None
        }
    })
}

pub fn resolve_trailer_youtube_id(item: &MediaItem) -> Option<String> {
    if let Some(yt) = item
        .trailer_youtube_id
        .as_deref()
        .filter(|s| is_valid_youtube_id(s))
    {
        return Some(yt.trim().to_string());
    }
    if let Some(enriched) = enrich_item_details(item) {
        if let Some(yt) = enriched
            .trailer_youtube_id
            .as_deref()
            .filter(|s| is_valid_youtube_id(s))
        {
            return Some(yt.trim().to_string());
        }
    }

    let type_hint = match item.media_type {
        MediaType::Movie => "movie",
        MediaType::Anime => "anime",
        MediaType::Show => "series",
    };
    let year_hint: String = item
        .year
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let q = if year_hint.len() == 4 {
        format!("{} {} {} official trailer", item.title, year_hint, type_hint)
    } else {
        format!("{} {} official trailer", item.title, type_hint)
    };
    let url = format!(
        "https://www.youtube.com/results?search_query={}",
        url_encode(&q)
    );
    let agent = http_agent();
    let resp = agent
        .get(&url)
        .set(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        )
        .set("Accept-Language", "en-US,en;q=0.9")
        .call()
        .ok()?;
    let html = resp.into_string().ok()?;
    extract_first_video_id(&html)
}

fn extract_first_video_id(html: &str) -> Option<String> {
    let needle = "\"videoId\":\"";
    let mut rest = html;
    while let Some(pos) = rest.find(needle) {
        let after = &rest[pos + needle.len()..];
        if after.len() >= 12 && after.as_bytes()[11] == b'"' {
            let cand = &after[..11];
            if is_valid_youtube_id(cand) {
                return Some(cand.to_string());
            }
        }
        rest = &after[1..];
    }
    None
}

fn fetch_cinemeta_meta(
    agent: &ureq::Agent,
    item: &MediaItem,
    imdb: &str,
    seg: &str,
) -> Option<MediaItem> {
    let url = format!("{CINEMETA_BASE}/meta/{seg}/{imdb}.json");
    let root: Value = agent.get(&url).call().ok()?.into_json().ok()?;
    let meta = root.get("meta")?;
    let parsed = parse_stremio_meta(meta, item.media_type, &item.source_badge)?;
    if parsed.ids.imdb.as_deref().is_some_and(|id| id != imdb) {
        return None;
    }

    let seasons_count = meta
        .get("videos")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let mut seasons = HashSet::new();
            for v in arr {
                if let Some(s) = v.get("season").and_then(|n| n.as_i64()) {
                    if s > 0 {
                        seasons.insert(s as u32);
                    }
                }
            }
            seasons.len() as u32
        })
        .filter(|&c| c > 0);

    Some(MediaItem {
        id: item.id.clone(),
        media_type: item.media_type,
        title: item.title.clone(),
        year: if parsed.year.is_empty() {
            item.year.clone()
        } else {
            parsed.year
        },
        status: if parsed.status.is_empty() {
            item.status.clone()
        } else {
            parsed.status
        },
        imdb_rating: if parsed.imdb_rating.is_empty() {
            item.imdb_rating.clone()
        } else {
            parsed.imdb_rating
        },
        runtime: if parsed.runtime.is_empty() {
            item.runtime.clone()
        } else {
            parsed.runtime
        },
        genres: if parsed.genres.is_empty() {
            item.genres.clone()
        } else {
            parsed.genres
        },
        overview: if parsed.overview.is_empty() {
            item.overview.clone()
        } else {
            parsed.overview
        },
        cast: if parsed.cast.is_empty() {
            item.cast.clone()
        } else {
            parsed.cast
        },
        creators: if parsed.creators.is_empty() {
            item.creators.clone()
        } else {
            parsed.creators
        },
        awards: parsed.awards.or_else(|| item.awards.clone()),
        network: item.network.clone(),
        seasons_count: seasons_count.or(item.seasons_count),
        poster_url: if item.poster_url.is_empty() {
            parsed.poster_url
        } else {
            item.poster_url.clone()
        },
        backdrop_url: if item.backdrop_url.is_empty() {
            parsed.backdrop_url
        } else {
            item.backdrop_url.clone()
        },
        logo_url: item.logo_url.clone().or(parsed.logo_url),
        trailer_youtube_id: item
            .trailer_youtube_id
            .clone()
            .or(parsed.trailer_youtube_id),
        ids: item.ids.merge_with(&parsed.ids),
        source_badge: item.source_badge.clone(),
    })
}

fn build_cinemeta_urls(
    media_type: MediaType,
    category: FeedCategory,
    genre: &str,
    page_index: usize,
) -> Vec<String> {
    let seg = if media_type == MediaType::Movie {
        "movie"
    } else {
        "series"
    };
    let has_genre = genre != "All" && !genre.trim().is_empty();
    let enc_genre = if has_genre { url_encode(genre) } else { String::new() };
    let skip = page_index * 50;

    match category {
        FeedCategory::NewReleases => {
            if !has_genre {
                let current_year = 2026usize;
                if page_index == 0 {
                    vec![
                        format!("{CINEMETA_BASE}/catalog/{seg}/year.json"),
                        format!("{CINEMETA_BASE}/catalog/{seg}/year/genre={current_year}.json"),
                    ]
                } else if page_index <= 15 {
                    let target = current_year.saturating_sub(page_index);
                    vec![format!("{CINEMETA_BASE}/catalog/{seg}/year/genre={target}.json")]
                } else {
                    Vec::new()
                }
            } else {
                let suffix = if skip > 0 {
                    format!("genre={enc_genre}&skip={skip}")
                } else {
                    format!("genre={enc_genre}")
                };
                vec![format!("{CINEMETA_BASE}/catalog/{seg}/top/{suffix}.json")]
            }
        }
        FeedCategory::TopRated => {
            let mut parts = Vec::new();
            if has_genre {
                parts.push(format!("genre={enc_genre}"));
            }
            if skip > 0 {
                parts.push(format!("skip={skip}"));
            }
            let suffix = if parts.is_empty() {
                String::new()
            } else {
                format!("/{}", parts.join("&"))
            };
            vec![format!("{CINEMETA_BASE}/catalog/{seg}/imdbRating{suffix}.json")]
        }
        FeedCategory::Trending | FeedCategory::ForYou => {
            let mut parts = Vec::new();
            if has_genre {
                parts.push(format!("genre={enc_genre}"));
            }
            if skip > 0 {
                parts.push(format!("skip={skip}"));
            }
            let suffix = if parts.is_empty() {
                String::new()
            } else {
                format!("/{}", parts.join("&"))
            };
            vec![format!("{CINEMETA_BASE}/catalog/{seg}/top{suffix}.json")]
        }
    }
}

fn build_anime_urls(category: FeedCategory, page_index: usize) -> Vec<String> {
    if page_index == 0 {
        match category {
            FeedCategory::Trending | FeedCategory::ForYou => vec![
                format!("{KITSU_BASE}/catalog/anime/kitsu-anime-trending.json"),
                format!("{KITSU_BASE}/catalog/anime/kitsu-anime-popular.json"),
            ],
            FeedCategory::TopRated => vec![
                format!("{KITSU_BASE}/catalog/anime/kitsu-anime-rating.json"),
                format!("{KITSU_BASE}/catalog/anime/kitsu-anime-list.json"),
            ],
            FeedCategory::NewReleases => vec![
                format!("{KITSU_BASE}/catalog/anime/kitsu-anime-airing.json"),
                format!("{KITSU_BASE}/catalog/anime/kitsu-anime-trending.json"),
            ],
        }
    } else if page_index <= 12 {
        let kitsu_skip = page_index * 20;
        let anim_skip = (page_index - 1) * 50;
        let suffix = if anim_skip > 0 {
            format!("genre=Animation&skip={anim_skip}")
        } else {
            "genre=Animation".to_string()
        };
        vec![
            format!("{KITSU_BASE}/catalog/anime/kitsu-anime-list/skip={kitsu_skip}.json"),
            format!("{CINEMETA_BASE}/catalog/series/top/{suffix}.json"),
        ]
    } else {
        Vec::new()
    }
}

fn fetch_cinemeta_catalog(
    media_type: MediaType,
    category: FeedCategory,
    genre: &str,
    page_index: usize,
) -> Vec<MediaItem> {
    let urls = build_cinemeta_urls(media_type, category, genre, page_index);
    if urls.is_empty() {
        return Vec::new();
    }
    let agent = http_agent();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for url in urls {
        for item in fetch_stremio_metas(&agent, &url, media_type, "IMDb · 4K") {
            if seen.insert(item.id.clone()) {
                out.push(item);
            }
        }
    }
    if category == FeedCategory::NewReleases && genre != "All" && !genre.trim().is_empty() {
        out.sort_by_key(|b| std::cmp::Reverse(extract_latest_year(&b.year)));
    }
    out
}

fn fetch_anime_catalog(category: FeedCategory, page_index: usize) -> Vec<MediaItem> {
    let urls = build_anime_urls(category, page_index);
    if urls.is_empty() {
        return Vec::new();
    }
    let agent = http_agent();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for url in urls {
        for item in fetch_stremio_metas(&agent, &url, MediaType::Anime, "Anime · 4K") {
            if seen.insert(item.id.clone()) {
                out.push(item);
            }
        }
    }
    out
}

fn fetch_stremio_metas(
    agent: &ureq::Agent,
    url: &str,
    media_type: MediaType,
    badge: &str,
) -> Vec<MediaItem> {
    let Ok(resp) = agent.get(url).call() else {
        return Vec::new();
    };
    let Ok(root) = resp.into_json::<Value>() else {
        return Vec::new();
    };
    let Some(metas) = root.get("metas").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    metas
        .iter()
        .filter_map(|elem| parse_stremio_meta(elem, media_type, badge))
        .collect()
}

fn opt_str(obj: &Value, key: &str) -> Option<String> {
    match obj.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn opt_i64(obj: &Value, key: &str) -> Option<i64> {
    match obj.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn str_list(obj: &Value, key: &str, max: usize) -> Vec<String> {
    obj.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .take(max)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_stremio_meta(obj: &Value, media_type: MediaType, badge: &str) -> Option<MediaItem> {
    let title = opt_str(obj, "name")?;
    let raw_id = opt_str(obj, "id");
    let imdb = opt_str(obj, "imdb_id")
        .filter(|s| s.starts_with("tt"))
        .or_else(|| raw_id.clone().filter(|s| s.starts_with("tt")));
    let tvdb = opt_i64(obj, "tvdb_id");
    let tmdb = opt_i64(obj, "moviedb_id");
    let slug = opt_str(obj, "slug").or_else(|| raw_id.filter(|s| !s.starts_with("tt")));

    let year = opt_str(obj, "releaseInfo")
        .or_else(|| opt_str(obj, "year"))
        .unwrap_or_default();
    let imdb_rating = opt_str(obj, "imdbRating").unwrap_or_default();
    let runtime = opt_str(obj, "runtime").unwrap_or_default();
    let status = opt_str(obj, "status").unwrap_or_default();
    let overview = opt_str(obj, "description").unwrap_or_default();
    let awards = opt_str(obj, "awards");

    let mut genres = str_list(obj, "genres", 8);
    if genres.is_empty() {
        genres = str_list(obj, "genre", 8);
    }
    let cast = str_list(obj, "cast", 6);
    let mut creators = str_list(obj, "director", 3);
    if creators.is_empty() {
        creators = str_list(obj, "writer", 3);
    }

    let mut trailer_yt = None;
    if let Some(arr) = obj.get("trailers").and_then(|v| v.as_array()) {
        for tr in arr {
            if let Some(src) = opt_str(tr, "source") {
                if is_valid_youtube_id(&src) {
                    trailer_yt = Some(src.trim().to_string());
                    if opt_str(tr, "type")
                        .map(|t| t.eq_ignore_ascii_case("trailer"))
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            }
        }
    }
    if trailer_yt.is_none() {
        if let Some(arr) = obj.get("trailerStreams").and_then(|v| v.as_array()) {
            for tr in arr {
                if let Some(yt) = opt_str(tr, "ytId") {
                    if is_valid_youtube_id(&yt) {
                        trailer_yt = Some(yt.trim().to_string());
                        break;
                    }
                }
            }
        }
    }

    let raw_poster = opt_str(obj, "poster").unwrap_or_default();
    let raw_bg = opt_str(obj, "background").unwrap_or_default();
    let poster_url = if imdb.is_some() {
        metahub_poster(imdb.as_deref())
    } else {
        raw_poster
    };
    let backdrop_url = if imdb.is_some() {
        metahub_backdrop(imdb.as_deref())
    } else if !raw_bg.is_empty() {
        raw_bg
    } else {
        poster_url.clone()
    };
    let logo_url = if imdb.is_some() {
        metahub_logo(imdb.as_deref())
    } else {
        opt_str(obj, "logo")
    };

    let ids = ExternalIds {
        imdb,
        tmdb,
        tvdb,
        slug,
        ..Default::default()
    };
    Some(MediaItem {
        id: ids.canonical_key(&title, &year),
        media_type,
        title,
        year,
        status,
        imdb_rating,
        runtime,
        genres,
        overview,
        cast,
        creators,
        awards,
        network: None,
        seasons_count: None,
        poster_url,
        backdrop_url,
        logo_url,
        trailer_youtube_id: trailer_yt,
        ids,
        source_badge: badge.to_string(),
    })
}

fn extract_latest_year(year_str: &str) -> i32 {
    let mut best = 0i32;
    let bytes = year_str.as_bytes();
    if bytes.len() >= 4 {
        for w in bytes.windows(4) {
            if w.iter().all(|b| b.is_ascii_digit()) {
                if let Ok(s) = std::str::from_utf8(w) {
                    if let Ok(y) = s.parse::<i32>() {
                        if (1900..=2099).contains(&y) {
                            best = best.max(y);
                        }
                    }
                }
            }
        }
    }
    if best > 0 && (year_str.trim_end().ends_with('–') || year_str.trim_end().ends_with('-')) {
        best = best.max(2026);
    }
    best
}

pub fn fallback_seed_deck(media_type: MediaType) -> Vec<MediaItem> {
    let seeds: &[(&str, MediaType, &str, &str, &str, &[&str], &str, &str)] = &[
        (
            "tt11280740",
            MediaType::Show,
            "Severance",
            "2022–",
            "8.7",
            &["Sci-Fi", "Drama", "Mystery"],
            "Mark leads a team of office workers whose memories have been surgically divided between their work and personal lives.",
            "xEQP4VVuyrY",
        ),
        (
            "tt2788316",
            MediaType::Show,
            "Shōgun",
            "2024–",
            "8.6",
            &["Drama", "Adventure", "History"],
            "When a mysterious European ship is found marooned in a nearby fishing village, Lord Yoshii Toranaga discovers secrets that could tip the scales of power.",
            "yAN5uspO_hk",
        ),
        (
            "tt14452776",
            MediaType::Show,
            "The Bear",
            "2022–",
            "8.5",
            &["Drama", "Comedy"],
            "A young chef from the fine dining world returns to Chicago to run his family's sandwich shop amid chaos, grief, and culinary obsession.",
            "gBmkI4jlaIo",
        ),
        (
            "tt11126994",
            MediaType::Show,
            "Arcane",
            "2021–2024",
            "9.0",
            &["Animation", "Action", "Sci-Fi"],
            "Set in utopian Piltover and the oppressed underground of Zaun, the story follows the origins of two iconic League champions and the power that tears them apart.",
            "fXmAurh012s",
        ),
        (
            "tt9253284",
            MediaType::Show,
            "Andor",
            "2022–2025",
            "8.4",
            &["Sci-Fi", "Action", "Drama"],
            "Prequel series to Rogue One following Cassian Andor's tense spy-thriller journey as a burgeoning rebellion takes shape against the Empire.",
            "cKOegEuCcfw",
        ),
        (
            "tt14688458",
            MediaType::Show,
            "Silo",
            "2023–",
            "8.1",
            &["Sci-Fi", "Drama", "Mystery"],
            "Men and women live in a giant silo underground with strict regulations they believe are meant to protect them from a toxic and ruined world.",
            "8ZYhuvIv1pA",
        ),
        (
            "tt12637874",
            MediaType::Show,
            "Fallout",
            "2024–",
            "8.3",
            &["Sci-Fi", "Action", "Adventure"],
            "In a future, post-apocalyptic Los Angeles brought about by nuclear decimation, citizens must live in underground bunkers to survive.",
            "V-mugKDQDlg",
        ),
        (
            "tt5875444",
            MediaType::Show,
            "Slow Horses",
            "2022–",
            "8.2",
            &["Thriller", "Drama", "Crime"],
            "Follows a dysfunctional team of MI5 agents—and their obnoxious boss, Jackson Lamb—as they defend England from sinister forces.",
            "O9ZJChzPn0U",
        ),
        (
            "tt15239678",
            MediaType::Movie,
            "Dune: Part Two",
            "2024",
            "8.5",
            &["Sci-Fi", "Action", "Drama"],
            "Paul Atreides unites with Chani and the Fremen while seeking revenge against the conspirators who destroyed his family.",
            "Way9Dexny3w",
        ),
        (
            "tt15398776",
            MediaType::Movie,
            "Oppenheimer",
            "2023",
            "8.3",
            &["Drama", "Thriller"],
            "The story of American scientist J. Robert Oppenheimer and his role in the development of the atomic bomb.",
            "uYPbbksJxIg",
        ),
        (
            "tt9362722",
            MediaType::Movie,
            "Spider-Man: Across the Spider-Verse",
            "2023",
            "8.6",
            &["Animation", "Action", "Sci-Fi"],
            "Miles Morales catapults across the Multiverse, where he encounters a team of Spider-People charged with protecting its very existence.",
            "shW9i6k8cB0",
        ),
        (
            "tt6751668",
            MediaType::Movie,
            "Parasite",
            "2019",
            "8.5",
            &["Thriller", "Drama", "Comedy"],
            "Greed and class discrimination threaten the newly formed symbiotic relationship between the wealthy Park family and the destitute Kim clan.",
            "5xH0HfJHsaY",
        ),
        (
            "tt22248376",
            MediaType::Anime,
            "Frieren: Beyond Journey's End",
            "2023–",
            "8.9",
            &["Animation", "Fantasy", "Adventure"],
            "After the party of heroes defeated the Demon King, elven mage Frieren embarks on a new journey to understand humanity and the passage of time.",
            "ZEkwCGJ3o7M",
        ),
        (
            "tt2560140",
            MediaType::Anime,
            "Attack on Titan",
            "2013–2023",
            "9.1",
            &["Animation", "Action", "Drama"],
            "After his hometown is destroyed, young Eren Jaeger vows to cleanse the earth of the giant humanoid Titans.",
            "MGRm4IzK1SQ",
        ),
        (
            "tt19844504",
            MediaType::Anime,
            "Solo Leveling",
            "2024–",
            "8.4",
            &["Animation", "Action", "Fantasy"],
            "In a world of gifted hunters and monsters, a weak hunter gains a mysterious power that lets him level up without limits.",
            "bssSj4cKsrI",
        ),
        (
            "tt13616990",
            MediaType::Anime,
            "Chainsaw Man",
            "2022–",
            "8.3",
            &["Animation", "Action", "Horror"],
            "Following a betrayal, a young man left for dead is reborn as a powerful devil-human hybrid after merging with his pet devil.",
            "q15CRdE5Bv0",
        ),
    ];

    seeds
        .iter()
        .filter(|s| s.1 == media_type)
        .map(|&(imdb, mt, title, year, rating, genres, overview, trailer)| {
            let ids = ExternalIds {
                imdb: Some(imdb.to_string()),
                ..Default::default()
            };
            MediaItem {
                id: ids.canonical_key(title, year),
                media_type: mt,
                title: title.to_string(),
                year: year.to_string(),
                status: String::new(),
                imdb_rating: rating.to_string(),
                runtime: String::new(),
                genres: genres.iter().map(|s| (*s).to_string()).collect(),
                overview: overview.to_string(),
                cast: Vec::new(),
                creators: Vec::new(),
                awards: None,
                network: None,
                seasons_count: None,
                poster_url: metahub_poster(Some(imdb)),
                backdrop_url: metahub_backdrop(Some(imdb)),
                logo_url: metahub_logo(Some(imdb)),
                trailer_youtube_id: Some(trailer.to_string()),
                ids,
                source_badge: "Curated 4K".to_string(),
            }
        })
        .collect()
}
