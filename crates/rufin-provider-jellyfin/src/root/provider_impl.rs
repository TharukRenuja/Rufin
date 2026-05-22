fn lyrics_from_dto(track_id: TrackId, source: LyricsSource, dto: LyricDto) -> Lyrics {
    Lyrics {
        track_id,
        source,
        lines: dto
            .lyrics
            .unwrap_or_default()
            .into_iter()
            .filter_map(|line| {
                let text = line.text.unwrap_or_default();
                (!text.trim().is_empty()).then_some(LyricLine {
                    text,
                    start_millis: ticks_to_millis(line.start),
                })
            })
            .collect(),
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    server_name: Option<String>,
    local_address: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticateByNameRequest {
    username: String,
    #[serde(rename = "Pw")]
    password: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticationResult {
    access_token: String,
    server_id: Option<String>,
    user: JellyfinUser,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinUser {
    id: String,
    name: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct CreatePlaylistDto {
    name: String,
    ids: Vec<String>,
    user_id: Option<String>,
    media_type: Option<String>,
    is_public: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct UpdatePlaylistDto {
    name: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PlaylistCreationResult {
    id: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LyricDto {
    lyrics: Option<Vec<LyricLineDto>>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LyricLineDto {
    text: Option<String>,
    start: Option<i64>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RemoteLyricInfoDto {
    id: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct PlaybackReportDto {
    can_seek: bool,
    item_id: String,
    is_paused: bool,
    is_muted: bool,
    position_ticks: i64,
    volume_level: i32,
    play_method: &'static str,
    repeat_mode: &'static str,
    playback_order: &'static str,
    failed: bool,
}
impl PlaybackReportDto {
    fn from_report(report: PlaybackReport) -> Self {
        Self {
            can_seek: true,
            item_id: raw_item_id(report.track_id.as_str()).to_string(),
            is_paused: report.paused,
            is_muted: report.muted,
            position_ticks: i64::from(report.position_seconds) * 10_000_000,
            volume_level: i32::from(report.volume_percent.min(100)),
            play_method: "DirectPlay",
            repeat_mode: if report.repeat_one {
                "RepeatOne"
            } else if report.repeat_all {
                "RepeatAll"
            } else {
                "RepeatNone"
            },
            playback_order: if report.shuffle { "Shuffle" } else { "Default" },
            failed: report.failed,
        }
    }
}
