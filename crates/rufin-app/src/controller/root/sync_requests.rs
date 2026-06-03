use super::*;

use std::io::{self, Read};
use std::time::Duration;

const LRCLIB_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const LRCLIB_RESPONSE_MAX_BYTES: usize = 2 * 1024 * 1024;
pub(in crate::controller) const LOCAL_LYRICS_MAX_BYTES: usize = 2 * 1024 * 1024;

pub(in crate::controller) fn resolve_stream(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    server_id: &ServerId,
    track_id: &TrackId,
    playback_settings: &PlaybackSettings,
) -> Result<StreamDescriptor, String> {
    let saved = store
        .with_store(|store| store.saved_server(server_id))?
        .ok_or_else(|| "No matching saved server is saved.".to_string())?;
    if saved.server.provider == "fake" {
        return Ok(StreamDescriptor::new(format!(
            "fake://local/stream/{}",
            track_id.as_str()
        )));
    }
    if let Some(local_path) =
        local_audio_path_for_saved_track(store, &saved.server, server_id, track_id)
    {
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
            id: value.id,
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
        lrclib_automatic_search_urls(artist_name, track_name)?,
        artist_name,
        track_name,
    )
}
fn lrclib_search_with_urls(
    urls: Vec<reqwest::Url>,
    artist_name: &str,
    track_name: &str,
) -> Result<Vec<LyricsSearchResult>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(LRCLIB_REQUEST_TIMEOUT)
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
                    if seen.insert(result.id) {
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
        .timeout(LRCLIB_REQUEST_TIMEOUT)
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
pub(in crate::controller) fn lrclib_best_lyrics(
    entry: &QueueEntry,
) -> Result<Option<Lyrics>, String> {
    let mut errors = Vec::new();
    if let Some(url) = lrclib_get_url(&entry.artist, &entry.title, entry.duration_seconds)? {
        let client = reqwest::blocking::Client::builder()
            .timeout(LRCLIB_REQUEST_TIMEOUT)
            .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| error.to_string())?;
        match lrclib_fetch_get(&client, url) {
            Ok(Some(result)) => {
                if let Some(lyrics) =
                    lyrics_from_lrclib_search_result(entry.track_id.clone(), &result)
                {
                    return Ok(Some(lyrics));
                }
            }
            Ok(None) => {}
            Err(error) => errors.push(error),
        }
    }

    match lrclib_automatic_search(&entry.artist, &entry.title) {
        Ok(results) => Ok(lyrics_from_lrclib_results(entry, results)),
        Err(error) if errors.is_empty() => Err(error),
        Err(error) => {
            errors.push(error);
            Err(errors.join("; "))
        }
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
pub(in crate::controller) fn lyrics_from_lrclib_results(
    entry: &QueueEntry,
    results: Vec<LyricsSearchResult>,
) -> Option<Lyrics> {
    results
        .into_iter()
        .find_map(|result| lyrics_from_lrclib_search_result(entry.track_id.clone(), &result))
}
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
pub(in crate::controller) fn lrclib_automatic_search_urls(
    artist_name: &str,
    track_name: &str,
) -> Result<Vec<reqwest::Url>, String> {
    let artist_name = artist_name.trim();
    let track_name = track_name.trim();
    if artist_name.is_empty() || track_name.is_empty() {
        return Ok(Vec::new());
    }
    let mut urls = Vec::new();
    let mut combined_url = lrclib_search_base_url()?;
    combined_url
        .query_pairs_mut()
        .append_pair("q", &format!("{track_name} {artist_name}"));
    push_unique_lrclib_search_url(&mut urls, combined_url);

    let mut field_url = lrclib_search_base_url()?;
    {
        let mut query = field_url.query_pairs_mut();
        query.append_pair("track_name", track_name);
        query.append_pair("artist_name", artist_name);
    }
    push_unique_lrclib_search_url(&mut urls, field_url);
    Ok(urls)
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
    results.sort_by(|a, b| {
        lrclib_match_score(a, artist_name, track_name)
            .cmp(&lrclib_match_score(b, artist_name, track_name))
            .then_with(|| lrclib_has_synced_lyrics(b).cmp(&lrclib_has_synced_lyrics(a)))
            .then_with(|| lrclib_has_plain_lyrics(b).cmp(&lrclib_has_plain_lyrics(a)))
            .then_with(|| a.track_name.cmp(&b.track_name))
            .then_with(|| a.artist_name.cmp(&b.artist_name))
    });
}
pub(in crate::controller) fn lrclib_match_score(
    result: &LyricsSearchResult,
    artist_name: &str,
    track_name: &str,
) -> u16 {
    text_match_score(track_name, &result.track_name).saturating_mul(2)
        + text_match_score(artist_name, &result.artist_name)
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
pub(in crate::controller) fn lrclib_has_synced_lyrics(result: &LyricsSearchResult) -> bool {
    result
        .synced_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
}
pub(in crate::controller) fn lrclib_has_plain_lyrics(result: &LyricsSearchResult) -> bool {
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
) -> Result<(PathBuf, Lyrics), String> {
    let content = lyrics_result_content(result)
        .ok_or_else(|| "Selected lyric result has no lyrics to save.".to_string())?;
    let path = output_path;
    fs::write(&path, content).map_err(|error| error.to_string())?;
    let lyrics = lyrics_from_text(entry.track_id.clone(), result);
    debug!(server_id = %server_id, path = %path.display(), "saved lyric file");
    Ok((path, lyrics))
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
    let path = audio_path.with_extension("lrc");
    let content = read_text_file_bounded(&path, LOCAL_LYRICS_MAX_BYTES).ok()?;
    let lines = content
        .lines()
        .filter_map(lyric_line_from_text)
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| Lyrics {
        track_id: track_id.clone(),
        source: rufin_provider::LyricsSource::Local,
        lines,
    })
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
fn local_audio_path_for_saved_track(
    store: &StoreHandle,
    server: &ServerIdentity,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Option<PathBuf> {
    let lookup = store
        .with_store(|store| local_audio_path_lookup(store, server, server_id, track_id))
        .ok()
        .flatten()?;
    local_audio_path_from_lookup(&lookup)
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
        && matched.is_file()
    {
        return Some(matched);
    }
    let raw = lookup.raw_path.as_deref()?;
    let direct = PathBuf::from(&raw);
    if direct.is_file() {
        return Some(direct);
    }
    let mapped = map_server_path_to_local(raw, access)?;
    mapped.is_file().then_some(mapped)
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
        let suffix = raw[prefix.len()..].trim_start_matches(['/', '\\']);
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
