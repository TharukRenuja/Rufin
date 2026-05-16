use gettextrs::{
    LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, setlocale, textdomain,
};

const DOMAIN: &str = "rufin";

pub fn init() {
    let _locale = setlocale(LocaleCategory::LcAll, "");
    let localedir = std::env::var("RUFIN_LOCALEDIR").unwrap_or_else(|_| "po".to_string());
    let _domain_dir = bindtextdomain(DOMAIN, localedir);
    let _codeset = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _domain = textdomain(DOMAIN);
}

pub fn tr(message: &str) -> String {
    gettext(message)
}

#[allow(dead_code)]
fn catalog_strings_for_extraction() {
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
    let _ = tr("Playlist");
    let _ = tr("Playlists");
    let _ = tr("Search");
    let _ = tr("Settings");
    let _ = tr("Explore");
    let _ = tr("Most played");
    let _ = tr("Newly added");
    let _ = tr("Recently played");
    let _ = tr("Recently released");
    let _ = tr("Back");
    let _ = tr("Forward");
    let _ = tr("Menu");
    let _ = tr("Main Menu");
    let _ = tr("Preferences");
    let _ = tr("Keyboard Shortcuts");
    let _ = tr("Toggle Fullscreen");
    let _ = tr("About Rufin");
    let _ = tr("General");
    let _ = tr("Connect");
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
    let _ = tr("Auto DJ on");
    let _ = tr("Repeat off");
    let _ = tr("Repeat one");
    let _ = tr("Repeat all");
    let _ = tr("Show sidebar");
    let _ = tr("Hide sidebar");
    let _ = tr("Remove from queue");
    let _ = tr("Remove from Queue");
    let _ = tr("Mute");
    let _ = tr("Play now");
    let _ = tr("Play Now");
    let _ = tr("Play next");
    let _ = tr("Play Next");
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
    let _ = tr("Artist detail will use cached album and track groups.");
    let _ = tr("Genre detail keeps albums above tracks.");
    let _ = tr("Playlist detail will use the track table.");
    let _ = tr("Cached library data will appear here as sync pages finish.");
    let _ = tr("The selected cached album was not found.");
    let _ = tr("Cached rows will appear here after the background sync finishes.");
    let _ = tr("Type a query in the sidebar search field to search the local cache.");
    let _ = tr("Native GTK music client shell with Jellyfin playback.");
}
