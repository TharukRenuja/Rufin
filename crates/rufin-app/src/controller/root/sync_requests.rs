fn resolve_stream(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    server_id: &ServerId,
    track_id: &TrackId,
    playback_settings: &PlaybackSettings,
) -> Result<StreamDescriptor, String> {
    let saved = store
        .with_store(|store| store.active_server())?
        .filter(|saved| saved.server.id == *server_id)
        .ok_or_else(|| "No matching active server is saved.".to_string())?;
    if saved.server.provider == "fake" {
        return Ok(StreamDescriptor::new(format!(
            "fake://local/stream/{}",
            track_id.as_str()
        )));
    }
    if saved.server.provider != "local"
        && let Some(local_path) = local_audio_path_for_track(store, server_id, track_id)
    {
        let url = reqwest::Url::from_file_path(&local_path).map_err(|()| {
            format!(
                "Could not turn local track path into a file URI: {}",
                local_path.display()
            )
        })?;
        debug!(
            server_id = %server_id,
            track_id = %track_id.as_str(),
            path = %local_path.display(),
            "resolved remote track to local playback file"
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
struct LrcLibLyricsDto {
    id: u64,
    #[serde(default, alias = "name")]
    track_name: String,
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
            track_name: value.track_name,
            artist_name: value.artist_name,
            album_name: value.album_name.unwrap_or_default(),
            duration_seconds: value.duration.unwrap_or_default().round() as u32,
            synced_lyrics: value.synced_lyrics,
            plain_lyrics: value.plain_lyrics,
        }
    }
}
fn lrclib_search(artist_name: &str, track_name: &str) -> Result<Vec<LyricsSearchResult>, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    let mut had_success = false;
    let mut errors = Vec::new();
    for url in lrclib_search_urls(artist_name, track_name)? {
        match lrclib_fetch_search(&client, url) {
            Ok(batch) => {
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
fn lrclib_search_urls(artist_name: &str, track_name: &str) -> Result<Vec<reqwest::Url>, String> {
    let artist_name = artist_name.trim();
    let track_name = track_name.trim();
    let mut urls = Vec::new();
    let combined_query = [track_name, artist_name]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !combined_query.is_empty() {
        let mut url = lrclib_search_base_url()?;
        url.query_pairs_mut().append_pair("q", &combined_query);
        urls.push(url);
    }
    if !track_name.is_empty() {
        let mut url = lrclib_search_base_url()?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("track_name", track_name);
            if !artist_name.is_empty() {
                query.append_pair("artist_name", artist_name);
            }
        }
        urls.push(url);
    }
    Ok(urls)
}
fn lrclib_search_base_url() -> Result<reqwest::Url, String> {
    reqwest::Url::parse("https://lrclib.net/api/search").map_err(|error| error.to_string())
}
fn lrclib_fetch_search(
    client: &reqwest::blocking::Client,
    url: reqwest::Url,
) -> Result<Vec<LyricsSearchResult>, String> {
    let body = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("Lyric search failed: {error}"))?
        .text()
        .map_err(|error| format!("Lyric search failed: {error}"))?;
    parse_lrclib_search_body(&body)
}
fn parse_lrclib_search_body(body: &str) -> Result<Vec<LyricsSearchResult>, String> {
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
fn order_lrclib_results(results: &mut [LyricsSearchResult], artist_name: &str, track_name: &str) {
    results.sort_by(|a, b| {
        lrclib_match_score(a, artist_name, track_name)
            .cmp(&lrclib_match_score(b, artist_name, track_name))
            .then_with(|| lrclib_has_synced_lyrics(b).cmp(&lrclib_has_synced_lyrics(a)))
            .then_with(|| lrclib_has_plain_lyrics(b).cmp(&lrclib_has_plain_lyrics(a)))
            .then_with(|| a.track_name.cmp(&b.track_name))
            .then_with(|| a.artist_name.cmp(&b.artist_name))
    });
}
fn lrclib_match_score(result: &LyricsSearchResult, artist_name: &str, track_name: &str) -> u16 {
    text_match_score(track_name, &result.track_name).saturating_mul(2)
        + text_match_score(artist_name, &result.artist_name)
}
fn text_match_score(query: &str, candidate: &str) -> u16 {
    let query = normalize_search_text(query);
    if query.is_empty() {
        return 0;
    }
    let candidate = normalize_search_text(candidate);
    if candidate == query {
        return 0;
    }
    if candidate.contains(&query) || query.contains(&candidate) {
        return 10;
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
        return 100 + query_tokens.len() as u16 * 10;
    }
    (missing as u16 * 30) + (extra.min(6) as u16 * 4)
}
fn normalize_search_text(value: &str) -> String {
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
fn lrclib_has_synced_lyrics(result: &LyricsSearchResult) -> bool {
    result
        .synced_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
}
fn lrclib_has_plain_lyrics(result: &LyricsSearchResult) -> bool {
    result
        .plain_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
}
fn save_lrclib_result(
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
fn lyrics_result_content(result: &LyricsSearchResult) -> Option<&str> {
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
fn local_sidecar_lyrics(
    store: &StoreHandle,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Option<Lyrics> {
    let audio_path = local_audio_path_for_track(store, server_id, track_id)?;
    let path = audio_path.with_extension("lrc");
    let content = fs::read_to_string(path).ok()?;
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
fn local_audio_path_for_track(
    store: &StoreHandle,
    server_id: &ServerId,
    track_id: &TrackId,
) -> Option<PathBuf> {
    let saved = store
        .with_store(|store| {
            store.list_servers().map(|servers| {
                servers
                    .into_iter()
                    .find(|saved| saved.server.id == *server_id)
            })
        })
        .ok()
        .flatten()?;
    let raw = store
        .with_store(|store| store.track_local_path(server_id, track_id))
        .ok()
        .flatten();
    if saved.server.provider == "local" {
        let direct = PathBuf::from(raw?);
        return direct.is_file().then_some(direct);
    }
    let access = store
        .with_store(|store| store.server_local_access(server_id))
        .ok()
        .flatten()?;
    if let Some(matched) = store
        .with_store(|store| store.track_local_match_path(server_id, track_id))
        .ok()
        .flatten()
    {
        let matched = PathBuf::from(matched);
        if matched.is_file() {
            return Some(matched);
        }
    }
    let raw = raw?;
    let direct = PathBuf::from(&raw);
    if direct.is_file() {
        return Some(direct);
    }
    let mapped = map_server_path_to_local(&raw, &access)?;
    mapped.is_file().then_some(mapped)
}
fn map_server_path_to_local(raw: &str, access: &ServerLocalAccess) -> Option<PathBuf> {
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
fn path_from_server_suffix(suffix: &str) -> PathBuf {
    suffix
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<PathBuf>()
}
