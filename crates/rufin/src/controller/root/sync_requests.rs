use super::*;

use library::SourceObject;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const EXTERNAL_LYRICS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const LRCLIB_RESPONSE_MAX_BYTES: usize = 2 * 1024 * 1024;
const SLOW_STREAM_RESOLVE_STAGE_MS: u128 = 250;
pub(in crate::controller) const LOCAL_LYRICS_MAX_BYTES: usize = 2 * 1024 * 1024;

pub(in crate::controller) fn resolve_stream(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    server_id: &ServerId,
    track_id: &TrackId,
    playback_settings: &PlaybackSettings,
) -> Result<StreamDescriptor, String> {
    let started = Instant::now();
    let PlaybackStreamLookup {
        saved,
        cue_source,
        local_path,
    } = playback_stream_lookup(store, server_id, track_id)?;
    if saved.server.provider == "fake" {
        return Ok(StreamDescriptor::new(format!(
            "fake://local/stream/{}",
            track_id.as_str()
        )));
    }
    if let Some(source) = cue_source.as_ref()
        && let Some(stream) = cue_track_stream_from_source(source)?
    {
        return Ok(stream);
    }
    if let Some(local_path) = local_path {
        let url = reqwest::Url::from_file_path(&local_path).map_err(|()| {
            format!(
                "Could not turn local track path into a file URI: {}",
                local_path.display()
            )
        })?;
        debug!(
            server_id = %server_id,
            provider = %saved.server.provider,
            track_id = %track_id.as_str(),
            path = %local_path.display(),
            "resolved track to local playback file"
        );
        return Ok(StreamDescriptor::new(url.to_string()));
    }
    if saved.server.provider == LOCAL_PROVIDER_ID {
        return Err(format!(
            "Cached local source is missing for track {}. Resync the local library.",
            track_id.as_str()
        ));
    }

    if saved.server.provider == "jellyfin" {
        let stream =
            jellyfin_stream_descriptor(store, secrets, &saved, track_id, playback_settings)?;
        debug!(
            server_id = %server_id,
            provider = %saved.server.provider,
            track_id = %track_id.as_str(),
            elapsed_ms = started.elapsed().as_millis(),
            "resolved direct Jellyfin playback descriptor"
        );
        return Ok(stream);
    }

    let provider = provider_for_saved(store, runtime, secrets, &saved)?;
    runtime
        .block_on(
            provider
                .as_music_provider()
                .stream_with_request(&StreamRequest::new(
                    track_id.clone(),
                    playback_settings.stream_quality,
                )),
        )
        .map_err(|error| error.to_string())
}

fn jellyfin_stream_descriptor(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
    track_id: &TrackId,
    playback_settings: &PlaybackSettings,
) -> Result<StreamDescriptor, String> {
    let stage_started = Instant::now();
    let device_id = ensure_jellyfin_device_id(store)?;
    log_slow_stream_stage(
        "jellyfin-device-id",
        stage_started.elapsed(),
        &saved.server.id,
        track_id,
    );
    let stage_started = Instant::now();
    let token = secrets
        .load_token(&saved.server.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No saved token found for the active server.".to_string())?;
    log_slow_stream_stage(
        "jellyfin-token",
        stage_started.elapsed(),
        &saved.server.id,
        track_id,
    );
    let session = SavedProviderSession {
        server: saved.server.clone(),
        user_id: saved.user_id.clone(),
        username: saved.username.clone(),
        trust_invalid_cert: saved.trust_invalid_cert,
        access_token: token,
        device_id: Some(device_id),
    };
    jellyfin_stream_descriptor_from_saved_session(
        &session,
        &StreamRequest::new(track_id.clone(), playback_settings.stream_quality),
    )
    .map_err(|error| error.to_string())
}

fn log_slow_stream_stage(stage: &str, elapsed: Duration, server_id: &ServerId, track_id: &TrackId) {
    let elapsed_ms = elapsed.as_millis();
    if elapsed_ms > SLOW_STREAM_RESOLVE_STAGE_MS {
        info!(
            stage,
            elapsed_ms,
            server_id = %server_id,
            track_id = %track_id.as_str(),
            "slow playback stream resolve stage"
        );
    }
}

struct PlaybackStreamLookup {
    saved: SavedServer,
    cue_source: Option<SourceObject>,
    local_path: Option<PathBuf>,
}

fn playback_stream_lookup(
    store: &StoreHandle,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Result<PlaybackStreamLookup, String> {
    let stage_started = Instant::now();
    let lookup = store
        .with_store_fast(|store| {
            let Some(saved) = store.saved_server(server_id)? else {
                return Ok(None);
            };
            let cue_source = if saved.server.provider == LOCAL_PROVIDER_ID {
                store
                    .load_track_source_object(server_id, track_id)?
                    .filter(|source| source.source_kind == "cue_track")
            } else {
                None
            };
            let local_path = playback_audio_path(store, &saved.server, server_id, track_id)?;
            Ok(Some(PlaybackStreamLookup {
                saved,
                cue_source,
                local_path,
            }))
        })?
        .ok_or_else(|| "No matching saved server is saved.".to_string())?;
    log_slow_stream_stage(
        "cached-playback-source",
        stage_started.elapsed(),
        server_id,
        track_id,
    );
    Ok(lookup)
}

fn cue_track_stream_from_source(source: &SourceObject) -> Result<Option<StreamDescriptor>, String> {
    let Some(path) = source.source_path.as_deref().map(PathBuf::from) else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let start_millis = source
        .segment_start_ms
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or_default();
    let Some(end_millis) = source
        .segment_end_ms
        .and_then(|value| u64::try_from(value).ok())
    else {
        return Ok(None);
    };
    let url = reqwest::Url::from_file_path(&path).map_err(|()| {
        format!(
            "Could not turn cue source path into a file URI: {}",
            path.display()
        )
    })?;
    Ok(Some(
        StreamDescriptor::new(url.to_string()).with_source_window(start_millis, end_millis),
    ))
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::controller) struct LrcLibLyricsDto {
    id: u64,
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    album_name: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    synced_lyrics: Option<String>,
    #[serde(default)]
    plain_lyrics: Option<String>,
}
impl From<LrcLibLyricsDto> for LyricsSearchResult {
    fn from(value: LrcLibLyricsDto) -> Self {
        Self {
            provider: ExternalLyricsProvider::Lrclib,
            id: value.id.to_string(),
            track_name: if value.track_name.trim().is_empty() {
                value.name
            } else {
                value.track_name
            },
            artist_name: value.artist_name,
            album_name: value.album_name.unwrap_or_default(),
            duration_seconds: value.duration.unwrap_or_default().round() as u32,
            synced_lyrics: value.synced_lyrics,
            plain_lyrics: value.plain_lyrics,
        }
    }
}
#[derive(Debug, Deserialize)]
struct NeteaseSearchResponse {
    result: Option<NeteaseSearchResult>,
}
#[derive(Debug, Deserialize)]
struct NeteaseSearchResult {
    songs: Option<Vec<NeteaseSong>>,
}
#[derive(Debug, Deserialize)]
struct NeteaseSong {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    artists: Vec<NeteaseArtist>,
    #[serde(default)]
    album: Option<NeteaseAlbum>,
    #[serde(default)]
    duration: Option<u64>,
}
#[derive(Debug, Deserialize)]
struct NeteaseArtist {
    #[serde(default)]
    name: String,
}
#[derive(Debug, Deserialize)]
struct NeteaseAlbum {
    #[serde(default)]
    name: String,
}
#[derive(Debug, Deserialize)]
struct NeteaseLyricsResponse {
    lrc: Option<NeteaseLyricsBody>,
}
#[derive(Debug, Deserialize)]
struct NeteaseLyricsBody {
    lyric: Option<String>,
}
#[derive(Debug, Deserialize)]
struct GeniusSearchResponse {
    response: Option<GeniusResponseBody>,
}
#[derive(Debug, Deserialize)]
struct GeniusResponseBody {
    sections: Option<Vec<GeniusSection>>,
}
#[derive(Debug, Deserialize)]
struct GeniusSection {
    hits: Option<Vec<GeniusHit>>,
}
#[derive(Debug, Deserialize)]
struct GeniusHit {
    result: GeniusSong,
}
#[derive(Debug, Deserialize)]
struct GeniusSong {
    #[serde(default)]
    artist_names: String,
    #[serde(default)]
    full_title: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
}
#[derive(Debug, Deserialize)]
struct SimpMusicSearchResponse {
    data: Option<Vec<SimpMusicLyric>>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimpMusicLyric {
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    album_name: Option<String>,
    #[serde(default)]
    duration_seconds: Option<u32>,
    #[serde(default)]
    plain_lyric: Option<String>,
    #[serde(default)]
    song_title: String,
    #[serde(default)]
    synced_lyrics: Option<String>,
    #[serde(default)]
    video_id: String,
}
pub(in crate::controller) fn lrclib_search(
    artist_name: &str,
    track_name: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    lrclib_search_priority_urls(
        lrclib_search_urls(artist_name, track_name)?,
        artist_name,
        track_name,
    )
}
pub(in crate::controller) fn lrclib_automatic_search(
    artist_name: &str,
    track_name: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    lrclib_search_with_urls(
        lrclib_search_urls(artist_name, track_name)?,
        artist_name,
        track_name,
    )
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::controller) struct LyricsLookup {
    artist_names: Vec<String>,
    track_name: String,
    duration_seconds: u32,
}
impl LyricsLookup {
    pub(in crate::controller) fn from_search(
        artist_name: &str,
        track_name: &str,
        duration_seconds: u32,
    ) -> Self {
        let mut lookup = Self {
            artist_names: Vec::new(),
            track_name: track_name.trim().to_string(),
            duration_seconds,
        };
        lookup.push_artist_name(artist_name);
        lookup.push_primary_artist_variants();
        lookup
    }

    fn from_entry(entry: &QueueEntry, track: Option<&Track>) -> Self {
        let mut lookup = Self::from_search(&entry.artist, &entry.title, entry.duration_seconds);
        if let Some(track) = track {
            if let Some(credit) = track.artist_credits.first() {
                lookup.push_artist_name(&credit.name);
            } else if let Some(credit) = track.album_artist_credits.first() {
                lookup.push_artist_name(&credit.name);
            }
            lookup.push_artist_name(&track.artist);
            lookup.push_primary_artist_variants();
        }
        lookup
    }

    fn queries(&self) -> Vec<(String, String)> {
        let artists = if self.artist_names.is_empty() {
            vec![String::new()]
        } else {
            self.artist_names.clone()
        };
        let mut queries = Vec::new();
        let mut seen = HashSet::new();
        for artist_name in artists {
            if self.track_name.is_empty() && artist_name.is_empty() {
                continue;
            }
            let key = (
                normalize_search_text(&artist_name),
                normalize_search_text(&self.track_name),
            );
            if seen.insert(key) {
                queries.push((artist_name, self.track_name.clone()));
            }
        }
        queries
    }

    fn push_artist_name(&mut self, artist_name: &str) {
        let artist_name = artist_name.trim();
        let normalized = normalize_search_text(artist_name);
        if artist_name.is_empty()
            || normalized.is_empty()
            || self
                .artist_names
                .iter()
                .any(|existing| normalize_search_text(existing) == normalized)
        {
            return;
        }
        self.artist_names.push(artist_name.to_string());
    }

    fn push_primary_artist_variants(&mut self) {
        let artists = self.artist_names.clone();
        for artist_name in artists {
            if let Some(primary) = primary_artist_name(&artist_name) {
                self.push_artist_name(&primary);
            }
        }
    }
}
fn lyrics_lookup_for_entry(
    store: &StoreHandle,
    server_id: &ServerId,
    entry: &QueueEntry,
) -> LyricsLookup {
    let track = match store.with_store(|store| store.load_track(server_id, &entry.track_id)) {
        Ok(track) => track,
        Err(error) => {
            debug!(
                track_id = %entry.track_id,
                %error,
                "could not load cached track credits for lyric lookup"
            );
            None
        }
    };
    LyricsLookup::from_entry(entry, track.as_ref())
}
fn primary_artist_name(artist_name: &str) -> Option<String> {
    const SEPARATORS: &[&str] = &[
        " • ",
        "•",
        " · ",
        "·",
        " / ",
        " | ",
        "; ",
        ";",
        " feat. ",
        " feat ",
        " featuring ",
        " ft. ",
        " ft ",
        " with ",
        " x ",
        " vs. ",
    ];
    let artist_name = artist_name.trim();
    let lower = artist_name.to_ascii_lowercase();
    let index = SEPARATORS
        .iter()
        .filter_map(|separator| {
            lower
                .find(&separator.to_ascii_lowercase())
                .map(|index| (index, separator.len()))
        })
        .min_by_key(|(index, _)| *index)
        .map(|(index, _)| index)?;
    let primary = artist_name.get(..index)?.trim();
    if primary.is_empty() || normalize_search_text(primary) == normalize_search_text(artist_name) {
        None
    } else {
        Some(primary.to_string())
    }
}
pub(in crate::controller) fn external_lyrics_search(
    providers: &[ExternalLyricsProvider],
    artist_name: &str,
    track_name: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    let lookup = LyricsLookup::from_search(artist_name, track_name, 0);
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let mut had_success = false;
    std::thread::scope(|scope| {
        let handles = providers
            .iter()
            .copied()
            .map(|provider| {
                let lookup = &lookup;
                scope.spawn(move || {
                    (
                        provider,
                        external_provider_search_for_lookup(provider, lookup, false),
                    )
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            let Ok((provider, result)) = handle.join() else {
                errors.push("lyric provider worker panicked".to_string());
                continue;
            };
            match result {
                Ok(mut batch) => {
                    had_success = true;
                    filter_external_results_for_lookup(&mut batch, &lookup);
                    order_external_provider_results(&mut batch, &lookup);
                    results.extend(batch);
                }
                Err(error) => errors.push(format!("{}: {error}", provider.title())),
            }
        }
    });
    if !had_success && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(results)
}
pub(in crate::controller) fn external_best_lyrics(
    store: &StoreHandle,
    server_id: &ServerId,
    entry: &QueueEntry,
    providers: &[ExternalLyricsProvider],
) -> Result<Option<Lyrics>, String> {
    let lookup = lyrics_lookup_for_entry(store, server_id, entry);
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let mut had_success = false;
    std::thread::scope(|scope| {
        let handles = providers
            .iter()
            .copied()
            .map(|provider| {
                let lookup = &lookup;
                scope.spawn(move || {
                    (
                        provider,
                        external_best_lyrics_for_provider(lookup, provider),
                    )
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            let Ok((provider, result)) = handle.join() else {
                errors.push("lyric provider worker panicked".to_string());
                continue;
            };
            match result {
                Ok(batch) => {
                    had_success = true;
                    if !batch.is_empty() {
                        results.extend(batch);
                    }
                }
                Err(error) => errors.push(format!("{}: {error}", provider.title())),
            }
        }
    });
    dedupe_external_results(&mut results);
    if results.is_empty() {
        if !had_success && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        return Ok(None);
    }
    filter_external_results_for_lookup(&mut results, &lookup);
    if results.is_empty() {
        return Ok(None);
    }
    order_external_results(&mut results, &lookup, providers);
    for result in results {
        match lyrics_from_search_result(entry.track_id.clone(), &result) {
            Ok(Some(lyrics)) => return Ok(Some(lyrics)),
            Ok(None) => {}
            Err(error) => errors.push(format!("{}: {error}", result.provider.title())),
        }
    }
    if !had_success && !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        Ok(None)
    }
}
fn external_best_lyrics_for_provider(
    lookup: &LyricsLookup,
    provider: ExternalLyricsProvider,
) -> Result<Vec<LyricsSearchResult>, String> {
    let mut results = Vec::new();
    if provider == ExternalLyricsProvider::Lrclib
        && let Some(result) = lrclib_exact_result(lookup)?
    {
        results.push(result);
    }
    results.extend(external_provider_search_for_lookup(provider, lookup, true)?);
    dedupe_external_results(&mut results);
    Ok(results)
}
#[cfg(test)]
pub(in crate::controller) fn filter_external_results_for_query(
    results: &mut Vec<LyricsSearchResult>,
    artist_name: &str,
    track_name: &str,
) {
    let lookup = LyricsLookup::from_search(artist_name, track_name, 0);
    filter_external_results_for_lookup(results, &lookup);
}
fn filter_external_results_for_lookup(
    results: &mut Vec<LyricsSearchResult>,
    lookup: &LyricsLookup,
) {
    results.retain(|result| external_result_matches_lookup(result, lookup));
}
fn external_result_matches_lookup(result: &LyricsSearchResult, lookup: &LyricsLookup) -> bool {
    if !lookup.track_name.is_empty()
        && text_match_score(&lookup.track_name, &result.track_name) > 70
    {
        return false;
    }
    if !lookup.artist_names.is_empty()
        && lookup
            .artist_names
            .iter()
            .map(|artist_name| text_match_score(artist_name, &result.artist_name))
            .min()
            .unwrap_or(0)
            > 80
    {
        return false;
    }
    true
}
fn lrclib_exact_result(lookup: &LyricsLookup) -> Result<Option<LyricsSearchResult>, String> {
    let Some((artist_name, track_name)) = lookup.queries().into_iter().next() else {
        return Ok(None);
    };
    let Some(url) = lrclib_get_url(&artist_name, &track_name, lookup.duration_seconds)? else {
        return Ok(None);
    };
    let client = external_lyrics_client(EXTERNAL_LYRICS_REQUEST_TIMEOUT)?;
    lrclib_fetch_get(&client, url)
}
fn external_provider_search_for_lookup(
    provider: ExternalLyricsProvider,
    lookup: &LyricsLookup,
    automatic: bool,
) -> Result<Vec<LyricsSearchResult>, String> {
    let queries = lookup.queries();
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    if queries.len() == 1 {
        let (artist_name, track_name) = &queries[0];
        return external_provider_search(provider, artist_name, track_name, automatic);
    }
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let mut had_success = false;
    std::thread::scope(|scope| {
        let handles = queries
            .into_iter()
            .map(|(artist_name, track_name)| {
                scope.spawn(move || {
                    external_provider_search(provider, &artist_name, &track_name, automatic)
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            match handle
                .join()
                .unwrap_or_else(|_| Err("Lyric search worker failed.".to_string()))
            {
                Ok(batch) => {
                    had_success = true;
                    results.extend(batch);
                }
                Err(error) => errors.push(error),
            }
        }
    });
    if !had_success && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    dedupe_external_results(&mut results);
    Ok(results)
}
fn dedupe_external_results(results: &mut Vec<LyricsSearchResult>) {
    let mut seen = HashSet::new();
    results.retain(|result| seen.insert((result.provider, result.id.clone())));
}
fn external_provider_search(
    provider: ExternalLyricsProvider,
    artist_name: &str,
    track_name: &str,
    automatic: bool,
) -> Result<Vec<LyricsSearchResult>, String> {
    match provider {
        ExternalLyricsProvider::Lrclib => {
            if automatic {
                lrclib_automatic_search(artist_name, track_name)
            } else {
                lrclib_search(artist_name, track_name)
            }
        }
        ExternalLyricsProvider::Netease => netease_search(artist_name, track_name),
        ExternalLyricsProvider::Genius => genius_search(artist_name, track_name),
        ExternalLyricsProvider::SimpMusic => simpmusic_search(artist_name, track_name),
    }
}
fn external_lyrics_client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())
}
fn netease_search(artist_name: &str, track_name: &str) -> Result<Vec<LyricsSearchResult>, String> {
    let query = [artist_name.trim(), track_name.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut url = reqwest::Url::parse("https://music.163.com/api/search/get")
        .map_err(|error| error.to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("s", &query);
        pairs.append_pair("type", "1");
        pairs.append_pair("limit", "5");
        pairs.append_pair("offset", "0");
    }
    let client = external_lyrics_client(EXTERNAL_LYRICS_REQUEST_TIMEOUT)?;
    let body = fetch_text(&client, url, "NetEase lyric search")?;
    parse_netease_search_body(&body)
}
pub(in crate::controller) fn parse_netease_search_body(
    body: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    let response = serde_json::from_str::<NeteaseSearchResponse>(body)
        .map_err(|error| format!("NetEase lyric search returned invalid data: {error}"))?;
    Ok(response
        .result
        .and_then(|result| result.songs)
        .unwrap_or_default()
        .into_iter()
        .filter(|song| !song.name.trim().is_empty() || !song.artists.is_empty())
        .map(|song| LyricsSearchResult {
            provider: ExternalLyricsProvider::Netease,
            id: song.id.to_string(),
            track_name: song.name,
            artist_name: song
                .artists
                .into_iter()
                .map(|artist| artist.name)
                .filter(|name| !name.trim().is_empty())
                .collect::<Vec<_>>()
                .join(", "),
            album_name: song.album.map(|album| album.name).unwrap_or_default(),
            duration_seconds: song.duration.unwrap_or_default().div_ceil(1000) as u32,
            synced_lyrics: None,
            plain_lyrics: None,
        })
        .collect())
}
fn netease_fetch_lyrics(id: &str) -> Result<Option<String>, String> {
    let mut url = reqwest::Url::parse("https://music.163.com/api/song/lyric")
        .map_err(|error| error.to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("id", id);
        pairs.append_pair("kv", "-1");
        pairs.append_pair("lv", "-1");
        pairs.append_pair("tv", "-1");
    }
    let client = external_lyrics_client(EXTERNAL_LYRICS_REQUEST_TIMEOUT)?;
    let body = fetch_text(&client, url, "NetEase lyric lookup")?;
    let response = serde_json::from_str::<NeteaseLyricsResponse>(&body)
        .map_err(|error| format!("NetEase lyric lookup returned invalid data: {error}"))?;
    Ok(response
        .lrc
        .and_then(|body| body.lyric)
        .filter(|lyrics| !lyrics.trim().is_empty()))
}
fn genius_search(artist_name: &str, track_name: &str) -> Result<Vec<LyricsSearchResult>, String> {
    let query = [artist_name.trim(), track_name.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut url = reqwest::Url::parse("https://genius.com/api/search/song")
        .map_err(|error| error.to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("q", &query);
        pairs.append_pair("per_page", "5");
    }
    let client = external_lyrics_client(EXTERNAL_LYRICS_REQUEST_TIMEOUT)?;
    let body = fetch_text(&client, url, "Genius lyric search")?;
    parse_genius_search_body(&body)
}
pub(in crate::controller) fn parse_genius_search_body(
    body: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    let response = serde_json::from_str::<GeniusSearchResponse>(body)
        .map_err(|error| format!("Genius lyric search returned invalid data: {error}"))?;
    let mut results = Vec::new();
    for section in response
        .response
        .and_then(|body| body.sections)
        .unwrap_or_default()
    {
        for hit in section.hits.unwrap_or_default() {
            if hit.result.url.trim().is_empty() {
                continue;
            }
            let track_name = if hit.result.full_title.trim().is_empty() {
                hit.result.title
            } else {
                hit.result.full_title
            };
            results.push(LyricsSearchResult {
                provider: ExternalLyricsProvider::Genius,
                id: hit.result.url,
                track_name,
                artist_name: hit.result.artist_names,
                album_name: String::new(),
                duration_seconds: 0,
                synced_lyrics: None,
                plain_lyrics: None,
            });
        }
    }
    Ok(results)
}
fn genius_fetch_lyrics(url: &str) -> Result<Option<String>, String> {
    let url = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
    let client = external_lyrics_client(EXTERNAL_LYRICS_REQUEST_TIMEOUT)?;
    let body = fetch_text(&client, url, "Genius lyric lookup")?;
    Ok(extract_genius_lyrics(&body).filter(|lyrics| !lyrics.trim().is_empty()))
}
fn simpmusic_search(
    artist_name: &str,
    track_name: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    let query = if !track_name.trim().is_empty() {
        track_name.trim().to_string()
    } else {
        artist_name.trim().to_string()
    };
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut url = reqwest::Url::parse("https://api-lyrics.simpmusic.org/v1/search")
        .map_err(|error| error.to_string())?;
    url.query_pairs_mut().append_pair("q", &query);
    let client = external_lyrics_client(Duration::from_secs(5))?;
    let body = fetch_text(&client, url, "SimpMusic lyric search")?;
    parse_simpmusic_search_body(&body)
}
pub(in crate::controller) fn parse_simpmusic_search_body(
    body: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    let response = serde_json::from_str::<SimpMusicSearchResponse>(body)
        .map_err(|error| format!("SimpMusic lyric search returned invalid data: {error}"))?;
    Ok(response
        .data
        .unwrap_or_default()
        .into_iter()
        .filter(|song| !song.video_id.trim().is_empty())
        .map(|song| LyricsSearchResult {
            provider: ExternalLyricsProvider::SimpMusic,
            id: song.video_id,
            track_name: song.song_title,
            artist_name: song.artist_name,
            album_name: song.album_name.unwrap_or_default(),
            duration_seconds: song.duration_seconds.unwrap_or_default(),
            synced_lyrics: song.synced_lyrics,
            plain_lyrics: song.plain_lyric,
        })
        .collect())
}
fn simpmusic_fetch_lyrics(id: &str) -> Result<Option<String>, String> {
    let url = reqwest::Url::parse(&format!("https://api-lyrics.simpmusic.org/v1/{id}"))
        .map_err(|error| error.to_string())?;
    let client = external_lyrics_client(Duration::from_secs(5))?;
    let body = fetch_text(&client, url, "SimpMusic lyric lookup")?;
    parse_simpmusic_lyrics_body(&body)
}
fn parse_simpmusic_lyrics_body(body: &str) -> Result<Option<String>, String> {
    if let Ok(song) = serde_json::from_str::<SimpMusicLyric>(body) {
        return Ok(song
            .synced_lyrics
            .filter(|lyrics| !lyrics.trim().is_empty())
            .or_else(|| song.plain_lyric.filter(|lyrics| !lyrics.trim().is_empty())));
    }
    let response = serde_json::from_str::<SimpMusicSearchResponse>(body)
        .map_err(|error| format!("SimpMusic lyric lookup returned invalid data: {error}"))?;
    Ok(response.data.and_then(|mut songs| {
        songs.drain(..).find_map(|song| {
            song.synced_lyrics
                .filter(|lyrics| !lyrics.trim().is_empty())
                .or_else(|| song.plain_lyric.filter(|lyrics| !lyrics.trim().is_empty()))
        })
    }))
}
fn fetch_text(
    client: &reqwest::blocking::Client,
    url: reqwest::Url,
    context: &str,
) -> Result<String, String> {
    let response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("{context} failed: {error}"))?;
    read_response_text_bounded(response, LRCLIB_RESPONSE_MAX_BYTES, context)
        .map_err(|error| format!("{context} failed: {error}"))
}
fn lrclib_search_with_urls(
    urls: Vec<reqwest::Url>,
    artist_name: &str,
    track_name: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(EXTERNAL_LYRICS_REQUEST_TIMEOUT)
        .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    let mut had_success = false;
    let mut errors = Vec::new();
    let handles = urls
        .into_iter()
        .map(|url| {
            let client = client.clone();
            thread::spawn(move || {
                debug!(url = %url, "requesting LRCLIB lyric search");
                lrclib_fetch_search(&client, url)
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        match handle
            .join()
            .unwrap_or_else(|_| Err("Lyric search worker failed.".to_string()))
        {
            Ok(batch) => {
                debug!(results = batch.len(), "received LRCLIB lyric search batch");
                had_success = true;
                for result in batch {
                    if seen.insert(result.id.clone()) {
                        results.push(result);
                    }
                }
            }
            Err(error) => errors.push(error),
        }
    }
    if !had_success && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    order_lrclib_results(&mut results, artist_name, track_name);
    Ok(results)
}
fn lrclib_search_priority_urls(
    urls: Vec<reqwest::Url>,
    artist_name: &str,
    track_name: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    if urls.is_empty() {
        return Ok(Vec::new());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(EXTERNAL_LYRICS_REQUEST_TIMEOUT)
        .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let mut errors = Vec::new();
    let mut had_success = false;
    let request_count = urls.len();
    let (sender, receiver) = channel();
    for url in urls {
        let client = client.clone();
        let sender = sender.clone();
        thread::spawn(move || {
            debug!(url = %url, "requesting LRCLIB lyric search");
            let _sent = sender.send(lrclib_fetch_search(&client, url));
        });
    }
    drop(sender);
    for _ in 0..request_count {
        match receiver
            .recv()
            .unwrap_or_else(|_| Err("Lyric search worker failed.".to_string()))
        {
            Ok(mut results) => {
                debug!(
                    results = results.len(),
                    "received LRCLIB lyric search batch"
                );
                had_success = true;
                if !results.is_empty() {
                    order_lrclib_results(&mut results, artist_name, track_name);
                    return Ok(results);
                }
            }
            Err(error) => errors.push(error),
        }
    }
    if had_success {
        Ok(Vec::new())
    } else {
        Err(errors.join("; "))
    }
}
pub(in crate::controller) fn lyrics_from_search_result(
    track_id: TrackId,
    result: &LyricsSearchResult,
) -> Result<Option<Lyrics>, String> {
    let content = match lyrics_result_content(result) {
        Some(content) => Some(content.to_string()),
        None => external_fetch_lyrics(result)?,
    };
    let Some(content) = content.filter(|lyrics| !lyrics.trim().is_empty()) else {
        return Ok(None);
    };
    let lyrics = lyrics_from_text_content(track_id, result.provider, &content);
    Ok(lyrics_with_displayable_content(lyrics))
}
fn external_fetch_lyrics(result: &LyricsSearchResult) -> Result<Option<String>, String> {
    match result.provider {
        ExternalLyricsProvider::Lrclib => Ok(None),
        ExternalLyricsProvider::Netease => netease_fetch_lyrics(&result.id),
        ExternalLyricsProvider::Genius => genius_fetch_lyrics(&result.id),
        ExternalLyricsProvider::SimpMusic => simpmusic_fetch_lyrics(&result.id),
    }
}
pub(in crate::controller) fn lrclib_get_url(
    artist_name: &str,
    track_name: &str,
    duration_seconds: u32,
) -> Result<Option<reqwest::Url>, String> {
    let artist_name = artist_name.trim();
    let track_name = track_name.trim();
    if artist_name.is_empty() || track_name.is_empty() {
        return Ok(None);
    }
    let mut url =
        reqwest::Url::parse("https://lrclib.net/api/get").map_err(|error| error.to_string())?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("track_name", track_name);
        query.append_pair("artist_name", artist_name);
        if duration_seconds > 0 {
            query.append_pair("duration", &duration_seconds.to_string());
        }
    }
    Ok(Some(url))
}
pub(in crate::controller) fn lrclib_fetch_get(
    client: &reqwest::blocking::Client,
    url: reqwest::Url,
) -> Result<Option<LyricsSearchResult>, String> {
    let response = match client.get(url).send() {
        Ok(response) => response,
        Err(error) => return Err(format!("Lyric lookup failed: {error}")),
    };
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("Lyric lookup failed: {error}"))?;
    let body = read_response_text_bounded(response, LRCLIB_RESPONSE_MAX_BYTES, "Lyric lookup")
        .map_err(|error| format!("Lyric lookup failed: {error}"))?;
    parse_lrclib_get_body(&body).map(Some)
}
pub(in crate::controller) fn parse_lrclib_get_body(
    body: &str,
) -> Result<LyricsSearchResult, String> {
    serde_json::from_str::<LrcLibLyricsDto>(body)
        .map(LyricsSearchResult::from)
        .map_err(|error| format!("Lyric lookup returned invalid data: {error}"))
}
#[cfg(test)]
pub(in crate::controller) fn lyrics_from_lrclib_results(
    entry: &QueueEntry,
    results: Vec<LyricsSearchResult>,
) -> Option<Lyrics> {
    results
        .into_iter()
        .find_map(|result| lyrics_from_lrclib_search_result(entry.track_id.clone(), &result))
}
#[cfg(test)]
pub(in crate::controller) fn lyrics_from_lrclib_search_result(
    track_id: TrackId,
    result: &LyricsSearchResult,
) -> Option<Lyrics> {
    lyrics_result_content(result)?;
    let lyrics = lyrics_from_text(track_id, result);
    (!lyrics.lines.is_empty()).then_some(lyrics)
}
pub(in crate::controller) fn lrclib_search_urls(
    artist_name: &str,
    track_name: &str,
) -> Result<Vec<reqwest::Url>, String> {
    let artist_name = artist_name.trim();
    let track_name = track_name.trim();
    let combined_query = [track_name, artist_name]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let combined_query = normalize_search_text(&combined_query);
    let mut urls = Vec::new();
    if !artist_name.is_empty() && !track_name.is_empty() {
        let mut field_url = lrclib_search_base_url()?;
        {
            let mut query = field_url.query_pairs_mut();
            query.append_pair("track_name", track_name);
            query.append_pair("artist_name", artist_name);
        }
        push_unique_lrclib_search_url(&mut urls, field_url);
    }
    if !combined_query.is_empty() {
        let mut url = lrclib_search_base_url()?;
        url.query_pairs_mut().append_pair("q", &combined_query);
        push_unique_lrclib_search_url(&mut urls, url);
    }
    if !track_name.is_empty()
        && let Some(short_artist_query) = shortened_artist_query(artist_name)
    {
        let short_query = normalize_search_text(&format!("{track_name} {short_artist_query}"));
        if !short_query.is_empty() {
            let mut url = lrclib_search_base_url()?;
            url.query_pairs_mut().append_pair("q", &short_query);
            push_unique_lrclib_search_url(&mut urls, url);
        }
    }
    Ok(urls)
}
fn shortened_artist_query(artist_name: &str) -> Option<String> {
    let normalized = normalize_search_text(artist_name);
    let mut tokens = normalized.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 2 {
        return None;
    }
    tokens.pop();
    Some(tokens.join(" "))
}
fn push_unique_lrclib_search_url(urls: &mut Vec<reqwest::Url>, url: reqwest::Url) {
    if urls
        .iter()
        .all(|existing| existing.as_str() != url.as_str())
    {
        urls.push(url);
    }
}
pub(in crate::controller) fn lrclib_search_base_url() -> Result<reqwest::Url, String> {
    reqwest::Url::parse("https://lrclib.net/api/search").map_err(|error| error.to_string())
}
pub(in crate::controller) fn lrclib_fetch_search(
    client: &reqwest::blocking::Client,
    url: reqwest::Url,
) -> Result<Vec<LyricsSearchResult>, String> {
    let response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("Lyric search failed: {error}"))?;
    let body = read_response_text_bounded(response, LRCLIB_RESPONSE_MAX_BYTES, "Lyric search")
        .map_err(|error| format!("Lyric search failed: {error}"))?;
    parse_lrclib_search_body(&body)
}
pub(in crate::controller) fn parse_lrclib_search_body(
    body: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    let values = serde_json::from_str::<Vec<serde_json::Value>>(body)
        .map_err(|error| format!("Lyric search returned invalid data: {error}"))?;
    let mut results = Vec::new();
    for value in values {
        match serde_json::from_value::<LrcLibLyricsDto>(value) {
            Ok(dto) => {
                let result = LyricsSearchResult::from(dto);
                if !result.track_name.trim().is_empty() || !result.artist_name.trim().is_empty() {
                    results.push(result);
                }
            }
            Err(error) => {
                debug!(%error, "skipped invalid LRCLIB search result");
            }
        }
    }
    Ok(results)
}
pub(in crate::controller) fn order_lrclib_results(
    results: &mut [LyricsSearchResult],
    artist_name: &str,
    track_name: &str,
) {
    let lookup = LyricsLookup::from_search(artist_name, track_name, 0);
    results.sort_by(|a, b| {
        lyrics_match_score(a, &lookup)
            .cmp(&lyrics_match_score(b, &lookup))
            .then_with(|| lrclib_has_synced_lyrics(b).cmp(&lrclib_has_synced_lyrics(a)))
            .then_with(|| lrclib_has_plain_lyrics(b).cmp(&lrclib_has_plain_lyrics(a)))
            .then_with(|| a.track_name.cmp(&b.track_name))
            .then_with(|| a.artist_name.cmp(&b.artist_name))
    });
}
pub(in crate::controller) fn order_external_provider_results(
    results: &mut [LyricsSearchResult],
    lookup: &LyricsLookup,
) {
    results.sort_by(|a, b| {
        lyrics_match_score(a, lookup)
            .cmp(&lyrics_match_score(b, lookup))
            .then_with(|| result_has_synced_lyrics(b).cmp(&result_has_synced_lyrics(a)))
            .then_with(|| result_has_plain_lyrics(b).cmp(&result_has_plain_lyrics(a)))
            .then_with(|| a.track_name.cmp(&b.track_name))
            .then_with(|| a.artist_name.cmp(&b.artist_name))
    });
}
fn order_external_results(
    results: &mut [LyricsSearchResult],
    lookup: &LyricsLookup,
    providers: &[ExternalLyricsProvider],
) {
    results.sort_by(|a, b| {
        lyrics_match_score(a, lookup)
            .cmp(&lyrics_match_score(b, lookup))
            .then_with(|| {
                provider_rank(a.provider, providers).cmp(&provider_rank(b.provider, providers))
            })
            .then_with(|| result_has_synced_lyrics(b).cmp(&result_has_synced_lyrics(a)))
            .then_with(|| result_has_plain_lyrics(b).cmp(&result_has_plain_lyrics(a)))
            .then_with(|| a.track_name.cmp(&b.track_name))
            .then_with(|| a.artist_name.cmp(&b.artist_name))
    });
}
fn provider_rank(provider: ExternalLyricsProvider, providers: &[ExternalLyricsProvider]) -> usize {
    providers
        .iter()
        .position(|candidate| *candidate == provider)
        .unwrap_or(usize::MAX)
}
fn lyrics_match_score(result: &LyricsSearchResult, lookup: &LyricsLookup) -> u16 {
    text_match_score(&lookup.track_name, &result.track_name)
        .saturating_mul(2)
        .saturating_add(artist_match_score(lookup, &result.artist_name))
        .saturating_add(duration_match_penalty(
            lookup.duration_seconds,
            result.duration_seconds,
        ))
}
fn artist_match_score(lookup: &LyricsLookup, artist_name: &str) -> u16 {
    lookup
        .artist_names
        .iter()
        .map(|query| text_match_score(query, artist_name))
        .min()
        .unwrap_or(0)
}
fn duration_match_penalty(target_seconds: u32, candidate_seconds: u32) -> u16 {
    if target_seconds == 0 || candidate_seconds == 0 {
        return 0;
    }
    let diff = target_seconds.abs_diff(candidate_seconds);
    match diff {
        0..=2 => 0,
        3..=5 => 4,
        6..=10 => 12,
        11..=20 => 30,
        _ => 60 + diff.min(90) as u16,
    }
}
pub(in crate::controller) fn text_match_score(query: &str, candidate: &str) -> u16 {
    let query = normalize_search_text(query);
    if query.is_empty() {
        return 0;
    }
    let candidate = normalize_search_text(candidate);
    if candidate == query {
        return 0;
    }
    let query_tokens = query.split_whitespace().collect::<HashSet<_>>();
    if query_tokens.is_empty() {
        return 0;
    }
    let candidate_tokens = candidate.split_whitespace().collect::<HashSet<_>>();
    let matched = query_tokens.intersection(&candidate_tokens).count();
    let missing = query_tokens.len().saturating_sub(matched);
    let extra = candidate_tokens.len().saturating_sub(matched);
    if matched == 0 {
        if candidate.contains(&query) || query.contains(&candidate) {
            return 10;
        }
        return 100 + query_tokens.len() as u16 * 10;
    }
    (missing as u16 * 30) + (extra.min(6) as u16 * 4)
}
pub(in crate::controller) fn normalize_search_text(value: &str) -> String {
    let mut normalized = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub(in crate::controller) fn extract_genius_lyrics(body: &str) -> Option<String> {
    let mut sections = Vec::new();
    let mut remaining = body;
    while let Some(marker_start) = remaining.find("data-lyrics-container=\"true\"") {
        let after_marker = remaining.get(marker_start..)?;
        let tag_end = after_marker.find('>')? + marker_start;
        let after_tag = remaining.get(tag_end + 1..)?;
        let section_end = after_tag.find("</div>").unwrap_or(after_tag.len());
        let section = strip_html_tags(after_tag.get(..section_end).unwrap_or_default());
        if !section.trim().is_empty() {
            sections.push(section);
        }
        remaining = after_tag
            .get(section_end.min(after_tag.len())..)
            .unwrap_or_default();
    }
    if sections.is_empty()
        && let Some(lyrics_start) = body.find("class=\"lyrics\"")
    {
        let after_marker = body.get(lyrics_start..)?;
        let tag_end = after_marker.find('>')? + lyrics_start;
        let after_tag = body.get(tag_end + 1..)?;
        let section_end = after_tag.find("</div>").unwrap_or(after_tag.len());
        let section = strip_html_tags(after_tag.get(..section_end).unwrap_or_default());
        if !section.trim().is_empty() {
            sections.push(section);
        }
    }
    let lyrics = sections.join("\n");
    (!lyrics.trim().is_empty()).then(|| lyrics.trim().to_string())
}
fn strip_html_tags(value: &str) -> String {
    let mut stripped = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for character in value.chars() {
        match character {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let tag_name = tag.trim().to_ascii_lowercase();
                if tag_name.starts_with("br") || tag_name.starts_with("/p") {
                    stripped.push('\n');
                }
            }
            _ if in_tag => tag.push(character),
            _ => stripped.push(character),
        }
    }
    decode_html_entities(&stripped)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
fn decode_html_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}
pub(in crate::controller) fn lrclib_has_synced_lyrics(result: &LyricsSearchResult) -> bool {
    result_has_synced_lyrics(result)
}
fn result_has_synced_lyrics(result: &LyricsSearchResult) -> bool {
    result
        .synced_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
}
pub(in crate::controller) fn lrclib_has_plain_lyrics(result: &LyricsSearchResult) -> bool {
    result_has_plain_lyrics(result)
}
fn result_has_plain_lyrics(result: &LyricsSearchResult) -> bool {
    result
        .plain_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
}
pub(in crate::controller) fn save_lrclib_result(
    server_id: &ServerId,
    entry: &QueueEntry,
    result: &LyricsSearchResult,
    output_path: PathBuf,
) -> Result<Option<(PathBuf, Lyrics)>, String> {
    let content = match lyrics_result_content(result) {
        Some(content) => Some(content.to_string()),
        None => external_fetch_lyrics(result)?,
    }
    .filter(|lyrics| !lyrics.trim().is_empty());
    let Some(content) = content else {
        return Ok(None);
    };
    let lyrics = lyrics_from_text_content(entry.track_id.clone(), result.provider, &content);
    let Some(lyrics) = lyrics_with_displayable_content(lyrics) else {
        return Ok(None);
    };
    let path = output_path;
    fs::write(&path, &content).map_err(|error| error.to_string())?;
    debug!(server_id = %server_id, path = %path.display(), "saved lyric file");
    Ok(Some((path, lyrics)))
}
pub(in crate::controller) fn lyrics_result_content(result: &LyricsSearchResult) -> Option<&str> {
    result
        .synced_lyrics
        .as_deref()
        .filter(|lyrics| !lyrics.trim().is_empty())
        .or_else(|| {
            result
                .plain_lyrics
                .as_deref()
                .filter(|lyrics| !lyrics.trim().is_empty())
        })
}
pub(in crate::controller) fn local_sidecar_lyrics(
    store: &StoreHandle,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Option<Lyrics> {
    let audio_path = local_audio_path_for_track(store, server_id, track_id)?;
    let cue_track = track_has_cue_source(store, server_id, track_id);
    let title = local_track_title(store, server_id, track_id);
    for path in local_sidecar_candidates(&audio_path, title.as_deref(), cue_track) {
        if let Some(lyrics) = lyrics_from_sidecar_file(track_id, &path) {
            return Some(lyrics);
        }
    }
    None
}
fn lyrics_from_sidecar_file(track_id: &TrackId, path: &Path) -> Option<Lyrics> {
    let content = read_text_file_bounded(path, LOCAL_LYRICS_MAX_BYTES).ok()?;
    let lines = content
        .lines()
        .filter_map(lyric_line_from_text)
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| Lyrics {
        track_id: track_id.clone(),
        source: source::LyricsSource::Local,
        external_provider: None,
        lines,
    })
}
fn local_sidecar_candidates(
    audio_path: &Path,
    title: Option<&str>,
    cue_track: bool,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if !cue_track {
        paths.push(audio_path.with_extension("lrc"));
    }
    if let Some(path) = title_matched_lrc(audio_path.parent(), title)
        && !paths.iter().any(|candidate| candidate == &path)
    {
        paths.push(path);
    }
    paths
}
fn title_matched_lrc(parent: Option<&Path>, title: Option<&str>) -> Option<PathBuf> {
    let parent = parent?;
    let title_key = normalized_lyrics_name(title?);
    if title_key.is_empty() {
        return None;
    }
    let mut matches = fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("lrc"))
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| normalized_lyrics_name(stem) == title_key)
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches.into_iter().next()
}
fn normalized_lyrics_name(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}
fn local_track_title(
    store: &StoreHandle,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Option<String> {
    store
        .with_store(|store| store.load_track(server_id, track_id))
        .ok()
        .flatten()
        .map(|track| track.title)
}
pub(in crate::controller) fn track_has_cue_source(
    store: &StoreHandle,
    server_id: &ServerId,
    track_id: &TrackId,
) -> bool {
    store
        .with_store(|store| store.load_track_source_object(server_id, track_id))
        .ok()
        .flatten()
        .is_some_and(|source| source.source_kind == "cue_track")
}
pub(in crate::controller) fn local_audio_path_for_track(
    store: &StoreHandle,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Option<PathBuf> {
    let lookup = store
        .with_store(|store| {
            let Some(saved) = store.saved_server(server_id)? else {
                return Ok(None);
            };
            local_audio_path_lookup(store, &saved.server, server_id, track_id)
        })
        .ok()
        .flatten()?;
    local_audio_path_from_lookup(&lookup)
}
fn playback_audio_path(
    store: &Store,
    server: &ServerIdentity,
    server_id: &ServerId,
    track_id: &TrackId,
) -> StoreResult<Option<PathBuf>> {
    if server.provider == LOCAL_PROVIDER_ID {
        return Ok(store
            .track_local_path(server_id, track_id)?
            .map(PathBuf::from));
    }
    Ok(store
        .track_local_match_path(server_id, track_id)?
        .map(PathBuf::from))
}
struct LocalAudioPathLookup {
    provider_is_local: bool,
    raw_path: Option<String>,
    access: Option<ServerLocalAccess>,
    matched_path: Option<String>,
}
fn local_audio_path_lookup(
    store: &Store,
    server: &ServerIdentity,
    server_id: &ServerId,
    track_id: &TrackId,
) -> StoreResult<Option<LocalAudioPathLookup>> {
    let raw_path = store.track_local_path(server_id, track_id)?;
    if server.provider == LOCAL_PROVIDER_ID {
        return Ok(Some(LocalAudioPathLookup {
            provider_is_local: true,
            raw_path,
            access: None,
            matched_path: None,
        }));
    }
    let Some(access) = store.server_local_access(server_id)? else {
        return Ok(None);
    };
    let matched_path = store.track_local_match_path(server_id, track_id)?;
    Ok(Some(LocalAudioPathLookup {
        provider_is_local: false,
        raw_path,
        access: Some(access),
        matched_path,
    }))
}
fn local_audio_path_from_lookup(lookup: &LocalAudioPathLookup) -> Option<PathBuf> {
    if lookup.provider_is_local {
        let direct = PathBuf::from(lookup.raw_path.as_deref()?);
        return direct.is_file().then_some(direct);
    }
    let access = lookup.access.as_ref()?;
    if let Some(matched) = lookup.matched_path.as_deref().map(PathBuf::from)
        && timed_is_file(&matched, "matched")
    {
        return Some(matched);
    }
    let raw = lookup.raw_path.as_deref()?;
    let direct = PathBuf::from(&raw);
    if timed_is_file(&direct, "raw") {
        return Some(direct);
    }
    let mapped = map_server_path_to_local(raw, access)?;
    timed_is_file(&mapped, "mapped").then_some(mapped)
}

fn timed_is_file(path: &Path, kind: &str) -> bool {
    let started = Instant::now();
    let exists = path.is_file();
    let elapsed_ms = started.elapsed().as_millis();
    if elapsed_ms > 250 {
        info!(kind, elapsed_ms, exists, "slow local audio file check");
    }
    exists
}
fn read_response_text_bounded(
    response: reqwest::blocking::Response,
    limit: usize,
    context: &str,
) -> Result<String, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(format!(
            "{context} exceeded {} MiB limit",
            bytes_to_mib(limit)
        ));
    }
    let bytes = read_bytes_bounded(response, limit, context).map_err(|error| error.to_string())?;
    String::from_utf8(bytes).map_err(|error| error.to_string())
}
fn read_text_file_bounded(path: &Path, limit: usize) -> io::Result<String> {
    if fs::metadata(path)?.len() > limit as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("lyrics file exceeded {} MiB limit", bytes_to_mib(limit)),
        ));
    }
    let file = fs::File::open(path)?;
    let bytes = read_bytes_bounded(file, limit, "lyrics file")?;
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
fn read_bytes_bounded<R: Read>(mut reader: R, limit: usize, context: &str) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|length| length > limit)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{context} exceeded {} MiB limit", bytes_to_mib(limit)),
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}
fn bytes_to_mib(bytes: usize) -> usize {
    bytes / 1024 / 1024
}
pub(in crate::controller) fn map_server_path_to_local(
    raw: &str,
    access: &ServerLocalAccess,
) -> Option<PathBuf> {
    let replace_to = access
        .path_replace_to
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&access.root_path);
    if let Some(prefix) = access
        .path_replace_from
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && raw.starts_with(prefix)
    {
        let suffix = raw.get(prefix.len()..)?.trim_start_matches(['/', '\\']);
        return Some(PathBuf::from(replace_to).join(path_from_server_suffix(suffix)));
    }
    let raw_path = Path::new(raw);
    if raw_path.is_relative() {
        return Some(PathBuf::from(replace_to).join(raw_path));
    }
    None
}
pub(in crate::controller) fn path_from_server_suffix(suffix: &str) -> PathBuf {
    suffix
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<PathBuf>()
}
