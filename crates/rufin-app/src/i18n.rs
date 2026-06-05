use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use gettextrs::{
    LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, setlocale, textdomain,
};
use rufin_core::{
    AppSettings, SYSTEM_LANGUAGE_PREFERENCE, default_language_preference,
    sanitize_language_preference,
};

const DOMAIN: &str = "rufin";
const SETTINGS_FILE_NAME: &str = "settings.json";
const ENGLISH_LANGUAGE_PREFERENCE: &str = "en";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageOption {
    pub id: String,
    pub title: String,
}

pub fn init(language_preference: &str) {
    apply_language_preference(language_preference);
    let localedir = locale_dir();
    let _domain_dir = bindtextdomain(DOMAIN, localedir);
    let _codeset = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _domain = textdomain(DOMAIN);
}

pub fn startup_language_preference() -> String {
    let Ok(value) = fs::read_to_string(app_settings_path()) else {
        return effective_language_preference(&default_language_preference());
    };
    let Ok(mut settings) = serde_json::from_str::<AppSettings>(&value) else {
        return effective_language_preference(&default_language_preference());
    };
    settings.migrate_defaults();
    effective_language_preference(&settings.language)
}

pub fn effective_language_preference(saved_language_preference: &str) -> String {
    if let Ok(value) = env::var("RUFIN_LANGUAGE") {
        return sanitize_language_preference(&value);
    }
    sanitize_language_preference(saved_language_preference)
}

pub fn language_options() -> Vec<LanguageOption> {
    let mut seen = BTreeSet::new();
    let mut options = Vec::new();
    options.push(LanguageOption {
        id: default_language_preference(),
        title: tr("System default"),
    });
    seen.insert(default_language_preference());
    options.push(LanguageOption {
        id: ENGLISH_LANGUAGE_PREFERENCE.to_string(),
        title: tr("English"),
    });
    seen.insert(ENGLISH_LANGUAGE_PREFERENCE.to_string());

    for id in available_translation_language_ids() {
        let id = sanitize_language_preference(&id);
        if id == SYSTEM_LANGUAGE_PREFERENCE || is_english_language(&id) || !seen.insert(id.clone())
        {
            continue;
        }
        options.push(LanguageOption {
            title: language_display_name(&id),
            id,
        });
    }

    options
}

pub fn language_option_index(options: &[LanguageOption], language_preference: &str) -> u32 {
    let language_preference = sanitize_language_preference(language_preference);
    let language_preference = if is_english_language(&language_preference) {
        ENGLISH_LANGUAGE_PREFERENCE
    } else {
        &language_preference
    };
    options
        .iter()
        .position(|option| option.id == language_preference)
        .unwrap_or_default() as u32
}

pub fn tr(message: &str) -> String {
    non_empty_translation(message, gettext(message))
}

pub fn set_language_preference(language_preference: &str) {
    apply_language_preference(language_preference);
}

fn apply_language_preference(language_preference: &str) {
    let _locale = setlocale(LocaleCategory::LcAll, "");
    let language_preference = sanitize_language_preference(language_preference);
    if language_preference == SYSTEM_LANGUAGE_PREFERENCE {
        return;
    }

    for candidate in locale_candidates(&language_preference) {
        if setlocale(LocaleCategory::LcMessages, candidate.as_str()).is_some() {
            return;
        }
    }
}

fn locale_candidates(language_preference: &str) -> Vec<String> {
    let language_preference = sanitize_language_preference(language_preference);
    if language_preference == SYSTEM_LANGUAGE_PREFERENCE {
        return Vec::new();
    }
    if is_english_language(&language_preference) {
        return vec!["C".to_string()];
    }

    let normalized = language_preference.replace('-', "_");
    let mut candidates = Vec::new();
    push_candidate(&mut candidates, language_preference);
    push_candidate(&mut candidates, normalized.clone());
    if !normalized.contains('.') {
        for codeset in ["UTF-8", "utf8"] {
            if let Some((base, modifier)) = normalized.split_once('@') {
                push_candidate(&mut candidates, format!("{base}.{codeset}@{modifier}"));
            } else {
                push_candidate(&mut candidates, format!("{normalized}.{codeset}"));
            }
        }
    }
    candidates
}

fn push_candidate(candidates: &mut Vec<String>, candidate: impl Into<String>) {
    let candidate = candidate.into();
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn is_english_language(language_preference: &str) -> bool {
    let language_preference = language_preference.replace('-', "_");
    matches!(language_preference.as_str(), "C" | "POSIX" | "en")
        || language_preference.starts_with("en_")
        || language_preference.starts_with("en.")
}

fn locale_dir() -> PathBuf {
    if let Some(path) = env::var_os("RUFIN_LOCALEDIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return path;
    }

    for candidate in locale_dir_candidates() {
        if candidate.exists() {
            return candidate;
        }
    }

    PathBuf::from("po")
}

fn locale_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = option_env!("RUFIN_BUILD_LOCALEDIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
    {
        candidates.push(path);
    }
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(PathBuf::from(manifest_dir).join("../../po"));
    }
    if let Ok(exe) = env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        candidates.push(exe_dir.join("share/locale"));
        candidates.push(exe_dir.join("../share/locale"));
    }
    candidates.push(PathBuf::from("po"));
    candidates
}

fn available_translation_language_ids() -> Vec<String> {
    let mut ids = BTreeSet::new();
    let localedir = locale_dir();
    collect_mo_language_ids(&localedir, &mut ids);
    collect_po_language_ids(&localedir, &mut ids);
    ids.into_iter().collect()
}

fn collect_mo_language_ids(localedir: &Path, ids: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(localedir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .join("LC_MESSAGES")
            .join(format!("{DOMAIN}.mo"))
            .is_file()
        {
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(sanitize_language_preference) else {
            continue;
        };
        if id != SYSTEM_LANGUAGE_PREFERENCE {
            ids.insert(id);
        }
    }
}

fn collect_po_language_ids(localedir: &Path, ids: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(localedir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("po") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem == DOMAIN {
            continue;
        }
        let id = sanitize_language_preference(stem);
        if id != SYSTEM_LANGUAGE_PREFERENCE {
            ids.insert(id);
        }
    }
}

fn language_display_name(id: &str) -> String {
    let code = language_code(id);
    let Some(name) = language_name(code) else {
        return id.to_string();
    };
    name.to_string()
}

fn language_code(id: &str) -> &str {
    id.split(['_', '-', '.', '@'])
        .next()
        .filter(|code| !code.is_empty())
        .unwrap_or(id)
}

fn language_name(code: &str) -> Option<&'static str> {
    match code {
        "ar" => Some("Arabic"),
        "bg" => Some("Bulgarian"),
        "ca" => Some("Catalan"),
        "cs" => Some("Czech"),
        "da" => Some("Danish"),
        "de" => Some("German"),
        "el" => Some("Greek"),
        "es" => Some("Spanish"),
        "eu" => Some("Basque"),
        "fa" => Some("Persian"),
        "fi" => Some("Finnish"),
        "fr" => Some("French"),
        "gl" => Some("Galician"),
        "he" => Some("Hebrew"),
        "hi" => Some("Hindi"),
        "hr" => Some("Croatian"),
        "hu" => Some("Hungarian"),
        "id" => Some("Indonesian"),
        "it" => Some("Italian"),
        "ja" => Some("Japanese"),
        "ko" => Some("Korean"),
        "lt" => Some("Lithuanian"),
        "lv" => Some("Latvian"),
        "ms" => Some("Malay"),
        "nb" => Some("Norwegian Bokmal"),
        "nl" => Some("Dutch"),
        "pl" => Some("Polish"),
        "pt" => Some("Portuguese"),
        "ro" => Some("Romanian"),
        "ru" => Some("Russian"),
        "sk" => Some("Slovak"),
        "sl" => Some("Slovenian"),
        "sr" => Some("Serbian"),
        "sv" => Some("Swedish"),
        "uk" => Some("Ukrainian"),
        "vi" => Some("Vietnamese"),
        "zh" => Some("Chinese"),
        _ => None,
    }
}

fn app_settings_path() -> PathBuf {
    ProjectDirs::from("io.github", "screwys", "Rufin")
        .map(|dirs| dirs.config_dir().join(SETTINGS_FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(SETTINGS_FILE_NAME))
}

fn non_empty_translation(message: &str, translated: String) -> String {
    if translated.is_empty() && !message.is_empty() {
        message.to_string()
    } else {
        translated
    }
}

#[allow(dead_code)]
fn catalog_strings_for_extraction() {
    let _ = tr("System default");
    let _ = tr("English");
    let _ = tr("Language");
    let _ = tr("Home");
    let _ = tr("Favorites");
    let _ = tr("Albums");
    let _ = tr("Album");
    let _ = tr("Tracks");
    let _ = tr("Artist");
    let _ = tr("Artists");
    let _ = tr("Album Artists");
    let _ = tr("Genre");
    let _ = tr("Genres");
    let _ = tr("Folders");
    let _ = tr("Folder");
    let _ = tr("Playlist");
    let _ = tr("Playlists");
    let _ = tr("Search");
    let _ = tr("Settings");
    let _ = tr("Explore");
    let _ = tr("Most played");
    let _ = tr("Newly added");
    let _ = tr("Recently played");
    let _ = tr("Recently released");
    let _ = tr("Favorite tracks");
    let _ = tr("Appears On");
    let _ = tr("appears on");
    let _ = tr("Discography");
    let _ = tr("View all tracks");
    let _ = tr("albums");
    let _ = tr("tracks");
    let _ = tr("Back");
    let _ = tr("Forward");
    let _ = tr("Menu");
    let _ = tr("Main Menu");
    let _ = tr("Preferences");
    let _ = tr("Keyboard Shortcuts");
    let _ = tr("Toggle Fullscreen");
    let _ = tr("About Rufin");
    let _ = tr("General");
    let _ = tr("Interface");
    let _ = tr("Library");
    let _ = tr("Actions");
    let _ = tr("Sync Status");
    let _ = tr("Metadata");
    let _ = tr("External lyric lookup");
    let _ = tr("External cover lookup");
    let _ = tr("Does MusicBrainz and Cover Art Archive lookups");
    let _ = tr("Try to fetch artwork from internet when server artwork is missing");
    let _ = tr("Clear Cached Library");
    let _ = tr("Clear Cache");
    let _ = tr("This removes cached library metadata for");
    let _ =
        tr("This removes the server, cached library metadata, queue snapshot, and saved token for");
    let _ = tr("Clears cached data for the server, login stays saved");
    let _ = tr("No active server");
    let _ = tr("Add Server");
    let _ = tr("Manage Server");
    let _ = tr("Configure local folder access");
    let _ = tr("Source");
    let _ = tr("Select Source");
    let _ = tr("No source");
    let _ = tr("Sources");
    let _ = tr("Servers");
    let _ = tr("Configure saved music servers and local playback mappings");
    let _ = tr("No servers configured");
    let _ = tr("Server Library");
    let _ = tr("All Music");
    let _ = tr("Manage");
    let _ = tr("Local Folders");
    let _ = tr("No local folders configured");
    let _ = tr("Add Local Folder");
    let _ = tr("Active Source Actions");
    let _ = tr("Map Local");
    let _ = tr("Edit Mapping");
    let _ = tr("Server Settings");
    let _ = tr("Server Actions");
    let _ = tr("Use This Source");
    let _ = tr("Selected Source");
    let _ = tr("Name");
    let _ = tr("Not set");
    let _ = tr("Yes");
    let _ = tr("No");
    let _ = tr("Local mapping");
    let _ = tr("No local playback mapping");
    let _ = tr("Local mapping saved. Sync to preview matches");
    let _ = tr("Saved per server");
    let _ = tr("None");
    let _ = tr("Use server streams only");
    let _ = tr("Choose Folder");
    let _ = tr("Add local folder access");
    let _ = tr("Remove Local Folder");
    let _ = tr("This removes the folder from the Local source");
    let _ = tr("Clear Mapping");
    let _ = tr("Save");
    let _ = tr("Save Mapping");
    let _ = tr("Save Server Settings");
    let _ = tr("Connect");
    let _ = tr("Connect to Music Server");
    let _ = tr("Preparing library…");
    let _ = tr("Library sync complete");
    let _ = tr("Choose a provider, pick a server, or enter the address manually");
    let _ = tr("Choose a provider, pick a discovered server, or enter the address manually");
    let _ = tr("Provider");
    let _ = tr("Username");
    let _ = tr("Password");
    let _ = tr("Jellyfin");
    let _ = tr("Navidrome");
    let _ = tr("Subsonic / OpenSubsonic");
    let _ = tr("Local");
    let _ = tr("Local Library");
    let _ = tr(
        "Choose a folder to scan and play locally from the computer (For flatpak users, if the folder is not in ~/Music, you need to give folder permissions from Flatseal)",
    );
    let _ = tr("Music Folder");
    let _ = tr("Music Folders");
    let _ = tr("No folder selected");
    let _ = tr("No folders selected");
    let _ = tr("folders selected");
    let _ = tr("Add");
    let _ = tr("Add Folder");
    let _ = tr("Add another folder to the Local source");
    let _ = tr("Choose one or more folders to scan and play directly from this computer");
    let _ = tr("Choose at least one local music folder");
    let _ = tr("Choose");
    let _ = tr("Local Playback Access");
    let _ = tr("Optionally map server tracks to files on this computer");
    let _ = tr("Local Folder");
    let _ = tr("Server Prefix");
    let _ = tr("Local Prefix");
    let _ = tr("Server Sample");
    let _ = tr("No cached server path yet");
    let _ = tr("Mapped Local Path");
    let _ = tr("Server sample does not match the server prefix.");
    let _ = tr("Enter a matching server prefix to map this path.");
    let _ = tr("Save to rescan this local library.");
    let _ = tr("Local library folder is saved.");
    let _ = tr("Choose a local prefix.");
    let _ = tr("Choose an existing local prefix.");
    let _ = tr("Enter a local prefix.");
    let _ = tr("Save to apply this mapping after the next sync.");
    let _ = tr("No cached tracks yet. Sync the server to preview matches.");
    let _ = tr("Unsaved changes.");
    let _ = tr("Saved mapping.");
    let _ = tr("Select Music Folder");
    let _ = tr("Choose a local music folder");
    let _ = tr("Enter a server address, username, and password");
    let _ = tr("Server Address");
    let _ = tr("Trust invalid certificates");
    let _ = tr("Trust invalid certificate");
    let _ = tr("Only use this for a server you control");
    let _ = tr("Found Servers");
    let _ = tr("Searching Local Network");
    let _ = tr("No Servers Found");
    let _ = tr("Search Again");
    let _ = tr("Resync Library");
    let _ = tr("Play");
    let _ = tr("Resume");
    let _ = tr("Pause");
    let _ = tr("Stop");
    let _ = tr("Previous");
    let _ = tr("Next");
    let _ = tr("Favorite");
    let _ = tr("Shuffle");
    let _ = tr("Shuffle on");
    let _ = tr("Auto DJ");
    let _ = tr("Auto DJ refill threshold");
    let _ = tr("Add tracks when fewer than this many remain");
    let _ = tr("Auto DJ on");
    let _ = tr("Play random");
    let _ = tr("Songs");
    let _ = tr("Minimum year");
    let _ = tr("Maximum year");
    let _ = tr("Any genre");
    let _ = tr("Play filter");
    let _ = tr("All tracks");
    let _ = tr("Only unplayed tracks");
    let _ = tr("Only played tracks");
    let _ = tr("Add Last");
    let _ = tr("Repeat off");
    let _ = tr("Repeat one");
    let _ = tr("Repeat all");
    let _ = tr("Show sidebar");
    let _ = tr("Hide sidebar");
    let _ = tr("Show lyrics");
    let _ = tr("Hide lyrics");
    let _ = tr("Search lyrics");
    let _ = tr("Search Lyrics");
    let _ = tr("Save Lyrics");
    let _ = tr("Close");
    let _ = tr("Disable automatic lyric search for this track");
    let _ = tr("No track playing");
    let _ = tr("Song");
    let _ = tr("Ready");
    let _ = tr("Enter an artist or song.");
    let _ = tr("Searching…");
    let _ = tr("No lyrics found.");
    let _ = tr("Search failed.");
    let _ = tr("results");
    let _ = tr("Synced lyrics");
    let _ = tr("Plain lyrics");
    let _ = tr("No lyrics");
    let _ = tr("Loaded in lyrics panel.");
    let _ = tr("Save");
    let _ = tr("Saved to");
    let _ = tr("Show Lyrics Panel");
    let _ = tr("Keep the lyrics section visible below the queue");
    let _ = tr("Keep the queue sidebar visible in the main window");
    let _ = tr("Prefer server lyrics");
    let _ = tr("Search server lyrics before external providers");
    let _ = tr("Use remote lyric providers when server lyrics are unavailable");
    let _ = tr("Scrobbling");
    let _ = tr("Last.fm");
    let _ = tr("Last.fm scrobbling");
    let _ = tr("MusicBrainz with Last.fm fallback");
    let _ = tr("Use listening activity");
    let _ = tr("Set the Discord activity type to Listening");
    let _ = tr("API keys");
    let _ = tr("If you do not have API keys, create them");
    let _ = tr("here");
    let _ = tr(". You only need ot fill email and an application name parts");
    let _ = tr("API key");
    let _ = tr("Shared secret");
    let _ = tr("Connection");
    let _ = tr("Connect");
    let _ = tr("Reconnect");
    let _ = tr("Not connected");
    let _ = tr("Connected");
    let _ = tr("Connected as");
    let _ = tr("Enter API credentials first");
    let _ = tr("Opening Last.fm authorization…");
    let _ = tr("Timed out waiting for Last.fm authorization.");
    let _ = tr("Now playing updates");
    let _ = tr("Libre.fm");
    let _ = tr("If the page doesn't load, then Libre.fm blocks your IP range/VPN");
    let _ = tr("Libre.fm scrobbling");
    let _ = tr("Opening Libre.fm authorization…");
    let _ = tr("Timed out waiting for Libre.fm authorization");
    let _ = tr("ListenBrainz");
    let _ = tr("ListenBrainz scrobbling");
    let _ = tr("Get token");
    let _ = tr("Find your ListenBrainz user token");
    let _ = tr("User token");
    let _ = tr(
        "Stop playback reporting, external lyrics, external metadata, notifications, and Discord IPC",
    );
    let _ = tr("Music Server");
    let _ = tr("Private mode is on");
    let _ = tr("App window");
    let _ = tr("Show tray icon");
    let _ = tr("Exit to tray");
    let _ = tr("Start minimized");
    let _ = tr("Show Rufin");
    let _ = tr("Play/Pause");
    let _ = tr("Previous Track");
    let _ = tr("Next Track");
    let _ = tr("Enable private mode");
    let _ = tr("Disable private mode");
    let _ = tr("Quit");
    let _ = tr("Rufin is running in the tray");
    let _ = tr("Left sidebar density");
    let _ = tr("Choose when the left sidebar uses compact navigation");
    let _ = tr("Plays");
    let _ = tr("Remove from playlist");
    let _ = tr("Remove from queue");
    let _ = tr("Remove from Queue");
    let _ = tr("Mute");
    let _ = tr("Seek");
    let _ = tr("Seekbar");
    let _ = tr("Queue and transitions");
    let _ = tr("Audio");
    let _ = tr("Skip same-album crossfade");
    let _ = tr("Keep album transitions gapless when possible");
    let _ = tr("Waveform seekbar");
    let _ = tr("Generate and cache waveforms for the current track");
    let _ = tr("Play now");
    let _ = tr("Play Now");
    let _ = tr("Play next");
    let _ = tr("Play Next");
    let _ = tr("Play Later");
    let _ = tr("Go to Artist");
    let _ = tr("Go to Album");
    let _ = tr("Add to queue");
    let _ = tr("Title");
    let _ = tr("Title (merged)");
    let _ = tr("Year");
    let _ = tr("Duration");
    let _ = tr("Yes");
    let _ = tr("Server");
    let _ = tr("No server");
    let _ = tr("Current server");
    let _ = tr("Local");
    let _ = tr("no account");
    let _ = tr("Nothing playing");
    let _ = tr("Queue a track to begin");
    let _ = tr("Open fullscreen player");
    let _ = tr("Close fullscreen player");
    let _ = tr("Queue");
    let _ = tr("No queue items match the search");
    let _ = tr("Artist detail will use cached album and track groups");
    let _ = tr("Genre detail keeps albums above tracks");
    let _ = tr("Playlist detail will use the track table");
    let _ = tr("Loading…");
    let _ = tr("Loading folders…");
    let _ = tr("Search current folder");
    let _ = tr("Artist / Album");
    let _ = tr("Folder browsing failed");
    let _ = tr("No folder contents found.");
    let _ = tr("Cached library data will appear here as sync pages finish");
    let _ = tr("The selected cached album was not found");
    let _ = tr("No cached results found");
    let _ = tr("Cached rows will appear here after the background sync finishes");
    let _ = tr("Type a query in the sidebar search field to search the local cache");
    let _ = tr(
        "Thank you for trying out Rufin! This a new app that is still in heavy development. If you have problems or suggestions, please open an issue in Github.",
    );
    let _ = tr("Website");
    let _ = tr("Issues");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i18n_include_variants() {
        assert_eq!(
            locale_candidates("de-DE"),
            vec!["de-DE", "de_DE", "de_DE.UTF-8", "de_DE.utf8"]
        );
    }

    #[test]
    fn i18n_locale_catalog() {
        assert_eq!(locale_candidates("en_US"), vec!["C"]);
    }

    #[test]
    fn i18n_language_system() {
        let options = vec![
            LanguageOption {
                id: SYSTEM_LANGUAGE_PREFERENCE.to_string(),
                title: "System default".to_string(),
            },
            LanguageOption {
                id: ENGLISH_LANGUAGE_PREFERENCE.to_string(),
                title: "English".to_string(),
            },
            LanguageOption {
                id: "de_DE".to_string(),
                title: "German".to_string(),
            },
        ];

        assert_eq!(language_option_index(&options, "C"), 1);
        assert_eq!(language_option_index(&options, "de_DE"), 2);
        assert_eq!(language_option_index(&options, "missing"), 0);
    }

    #[test]
    fn i18n_use_name() {
        assert_eq!(language_display_name("pt_BR"), "Portuguese");
        assert_eq!(language_display_name("de_DE"), "German");
        assert_eq!(language_display_name("zz_ZZ"), "zz_ZZ");
    }

    #[test]
    fn i18n_fall_empty() {
        assert_eq!(non_empty_translation("Previous", String::new()), "Previous");
        assert_eq!(non_empty_translation("", String::new()), "");
        assert_eq!(
            non_empty_translation("Play", "Translated Play".to_string()),
            "Translated Play"
        );
    }
}
