use super::*;
use std::collections::BTreeMap;

pub(super) fn build_library(
    scanned: Vec<ScannedTrack>,
    root_entries: Vec<LocalFolderEntry>,
    folders: HashMap<FolderId, LocalFolderEntry>,
) -> LocalLibrary {
    let mut albums = BTreeMap::<AlbumId, AlbumAccumulator>::new();
    let mut artists = BTreeMap::<ArtistId, ArtistAccumulator>::new();
    let mut album_artists = BTreeMap::<ArtistId, ArtistAccumulator>::new();
    let mut genres = BTreeMap::<GenreId, GenreAccumulator>::new();
    let mut covers = HashMap::new();
    let mut attempted_artist_cover_dirs = BTreeSet::<(ArtistId, PathBuf)>::new();
    let mut attempted_album_artist_cover_dirs = BTreeSet::<(ArtistId, PathBuf)>::new();
    let mut tracks = Vec::with_capacity(scanned.len());

    for mut scanned_track in scanned {
        let cover = scanned_track.cover.take();
        let embedded_cover_path = scanned_track.embedded_cover_path.take();
        let track = &mut scanned_track.track;
        let track_path = track.local_path.as_deref().map(Path::new);
        let album_entry =
            albums
                .entry(track.album_id.clone())
                .or_insert_with(|| AlbumAccumulator {
                    album: Album {
                        id: track.album_id.clone(),
                        title: track.album.clone(),
                        artist: scanned_track.album_artist.clone(),
                        artist_id: track
                            .album_artist_credits
                            .first()
                            .map(|artist| artist.id.clone()),
                        album_artist_credits: track.album_artist_credits.clone(),
                        artist_credits: track.artist_credits.clone(),
                        year: track.year,
                        release_date: track.release_date.clone(),
                        date_added: None,
                        last_played: None,
                        play_count: None,
                        user_rating: None,
                        track_count: 0,
                        duration_seconds: 0,
                        favorite: false,
                        color_seed: stable_hash(track.album_id.as_str()) as u32,
                        image_ref: None,
                        genres: Vec::new(),
                        release_types: Vec::new(),
                        is_compilation: None,
                        musicbrainz_album_id: scanned_track.musicbrainz_album_id.clone(),
                        musicbrainz_release_group_id: scanned_track
                            .musicbrainz_release_group_id
                            .clone(),
                    },
                    album_artist_keys: BTreeSet::new(),
                    artist_keys: BTreeSet::new(),
                    embedded_cover_path: None,
                });
        if album_entry.album.image_ref.is_none() {
            if let Some(cover) = cover {
                let cover_id = cover_id(&cover);
                let revision = cover_revision(&cover);
                covers.entry(cover_id.clone()).or_insert(cover);
                let image_ref = ImageRef::new(cover_id, revision);
                track.image_ref = Some(image_ref.clone());
                album_entry.album.image_ref = Some(image_ref);
            } else if let Some(image_ref) = track.image_ref.as_ref()
                && is_local_cover_ref(image_ref)
            {
                album_entry.album.image_ref = Some(image_ref.clone());
            } else if album_entry.embedded_cover_path.is_none() {
                album_entry.embedded_cover_path = embedded_cover_path;
            }
        }
        album_entry.album.track_count = album_entry.album.track_count.saturating_add(1);
        album_entry.album.duration_seconds = album_entry
            .album
            .duration_seconds
            .saturating_add(track.duration_seconds);
        if album_entry.album.year == 0 {
            album_entry.album.year = track.year;
        }
        if album_entry.album.musicbrainz_album_id.is_none() {
            album_entry.album.musicbrainz_album_id = scanned_track.musicbrainz_album_id.clone();
        }
        if album_entry.album.musicbrainz_release_group_id.is_none() {
            album_entry.album.musicbrainz_release_group_id =
                scanned_track.musicbrainz_release_group_id.clone();
        }
        merge_genres(&mut album_entry.album.genres, &track.genres);

        for artist in &track.artist_credits {
            album_entry
                .artist_keys
                .insert(artist.id.as_str().to_string());
            let artist_entry =
                artists
                    .entry(artist.id.clone())
                    .or_insert_with(|| ArtistAccumulator {
                        name: artist.name.clone(),
                        ..ArtistAccumulator::default()
                    });
            assign_local_artist_image_ref(
                &artist.id,
                &artist.name,
                track_path,
                artist_entry,
                &mut covers,
                &mut attempted_artist_cover_dirs,
            );
            artist_entry.tracks.insert(track.id.clone());
            artist_entry.albums.insert(track.album_id.clone());
        }
        for artist in &track.album_artist_credits {
            album_entry
                .album_artist_keys
                .insert(artist.id.as_str().to_string());
            let artist_entry =
                album_artists
                    .entry(artist.id.clone())
                    .or_insert_with(|| ArtistAccumulator {
                        name: artist.name.clone(),
                        ..ArtistAccumulator::default()
                    });
            assign_local_artist_image_ref(
                &artist.id,
                &artist.name,
                track_path,
                artist_entry,
                &mut covers,
                &mut attempted_album_artist_cover_dirs,
            );
            artist_entry.tracks.insert(track.id.clone());
            artist_entry.albums.insert(track.album_id.clone());
        }
        for genre_name in &track.genres {
            let genre_id = local_id("genre", genre_name);
            let genre = genres.entry(genre_id).or_insert_with(|| GenreAccumulator {
                name: genre_name.clone(),
                ..GenreAccumulator::default()
            });
            genre.albums.insert(track.album_id.clone());
            if genre.tracks.insert(track.id.clone()) {
                genre.duration_seconds = genre
                    .duration_seconds
                    .saturating_add(track.duration_seconds);
            }
        }
        tracks.push(track.clone());
    }

    for album_entry in albums.values_mut() {
        if album_entry.album.image_ref.is_some() {
            continue;
        }
        let Some(path) = album_entry.embedded_cover_path.as_ref() else {
            continue;
        };
        if let Some(cover) = embedded_cover_from_path(path) {
            let cover_id = cover_id(&cover);
            let revision = cover_revision(&cover);
            covers.entry(cover_id.clone()).or_insert(cover);
            album_entry.album.image_ref = Some(ImageRef::new(cover_id, revision));
        }
    }

    let album_image_refs = albums
        .iter()
        .filter_map(|(id, entry)| {
            entry
                .album
                .image_ref
                .clone()
                .map(|image_ref| (id.clone(), image_ref))
        })
        .collect::<HashMap<_, _>>();
    for track in &mut tracks {
        if track.image_ref.is_none() {
            track.image_ref = album_image_refs.get(&track.album_id).cloned();
        }
    }
    let track_image_refs = tracks
        .iter()
        .filter_map(|track| {
            track
                .image_ref
                .clone()
                .map(|image_ref| (track.id.clone(), image_ref))
        })
        .collect::<HashMap<_, _>>();
    for artist in artists.values_mut().chain(album_artists.values_mut()) {
        if artist.image_ref.is_none() {
            artist.image_ref =
                artist_fallback_image_ref(artist, &album_image_refs, &track_image_refs);
        }
    }

    let mut albums = albums
        .into_values()
        .map(|entry| entry.album)
        .collect::<Vec<_>>();
    albums.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then(left.artist.to_lowercase().cmp(&right.artist.to_lowercase()))
    });

    let mut artists = artists
        .into_iter()
        .map(|(id, artist)| artist_from_accumulator(id, artist))
        .collect::<Vec<_>>();
    artists.sort_by_key(|artist| artist.name.to_lowercase());

    let mut album_artists = album_artists
        .into_iter()
        .map(|(id, artist)| artist_from_accumulator(id, artist))
        .collect::<Vec<_>>();
    album_artists.sort_by_key(|artist| artist.name.to_lowercase());

    let mut genres = genres
        .into_iter()
        .map(|(id, genre)| Genre {
            id,
            name: genre.name,
            album_count: genre.albums.len().min(u32::MAX as usize) as u32,
            track_count: genre.tracks.len().min(u32::MAX as usize) as u32,
            duration_seconds: genre.duration_seconds,
            image_refs: Vec::new(),
            image_ref: None,
        })
        .collect::<Vec<_>>();
    genres.sort_by_key(|genre| genre.name.to_lowercase());

    LocalLibrary {
        roots: root_entries,
        folders,
        albums,
        tracks,
        artists,
        album_artists,
        genres,
        covers,
    }
}

fn assign_local_artist_image_ref(
    artist_id: &ArtistId,
    artist_name: &str,
    track_path: Option<&Path>,
    artist: &mut ArtistAccumulator,
    covers: &mut HashMap<String, LocalCover>,
    attempted_artist_cover_dirs: &mut BTreeSet<(ArtistId, PathBuf)>,
) {
    if artist.image_ref.is_some() {
        return;
    }
    let Some(track_path) = track_path else {
        return;
    };
    for (dir, allow_single_fallback) in artist_cover_dirs(track_path, artist_name) {
        if !attempted_artist_cover_dirs.insert((artist_id.clone(), dir.clone())) {
            continue;
        }
        let Some(path) = artist_cover_in_dir(&dir, artist_name, allow_single_fallback) else {
            continue;
        };
        let cover = local_file_cover(path);
        let cover_id = cover_id(&cover);
        let revision = cover_revision(&cover);
        covers.entry(cover_id.clone()).or_insert(cover);
        artist.image_ref = Some(ImageRef::new(cover_id, revision));
        return;
    }
}

fn artist_fallback_image_ref(
    artist: &ArtistAccumulator,
    album_image_refs: &HashMap<AlbumId, ImageRef>,
    track_image_refs: &HashMap<TrackId, ImageRef>,
) -> Option<ImageRef> {
    artist
        .albums
        .iter()
        .find_map(|id| album_image_refs.get(id).cloned())
        .or_else(|| {
            artist
                .tracks
                .iter()
                .find_map(|id| track_image_refs.get(id).cloned())
        })
}

fn is_local_cover_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with("local:cover:")
}

pub(super) fn artist_from_accumulator(id: ArtistId, artist: ArtistAccumulator) -> Artist {
    Artist {
        id,
        name: artist.name,
        album_count: artist.albums.len().min(u32::MAX as usize) as u32,
        track_count: artist.tracks.len().min(u32::MAX as usize) as u32,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: artist.image_ref,
    }
}
pub(super) fn page<T: Clone>(items: &[T], request: PagedRequest) -> PagedResponse<T> {
    let start = request.offset.min(items.len());
    let end = start.saturating_add(request.limit).min(items.len());
    PagedResponse::new(items[start..end].to_vec(), items.len())
}
pub(super) fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["mp3", "flac", "m4a", "wav", "ogg", "opus", "mp4", "mka"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}
pub(super) fn is_cue_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"))
}
pub(super) fn folder_cover(dir: &Path) -> Option<PathBuf> {
    let image_paths = folder_image_paths(dir);
    image_paths
        .iter()
        .filter_map(|path| explicit_folder_cover_rank(path).map(|rank| (rank, path.clone())))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, path)| path)
        .or_else(|| match image_paths.as_slice() {
            [path] => Some(path.clone()),
            _ => None,
        })
}

fn folder_image_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(dir)
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && supported_cover_extension(path))
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default()
    });
    paths
}

pub(super) fn supported_cover_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["jpg", "jpeg", "png", "webp"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn explicit_folder_cover_rank(path: &Path) -> Option<(usize, usize)> {
    let stem = path.file_stem().and_then(|stem| stem.to_str())?;
    let extension = path.extension().and_then(|extension| extension.to_str())?;
    let stem_rank = ["cover", "folder", "front", "album"]
        .iter()
        .position(|candidate| stem.eq_ignore_ascii_case(candidate))?;
    let extension_rank = ["jpg", "jpeg", "png", "webp"]
        .iter()
        .position(|candidate| extension.eq_ignore_ascii_case(candidate))?;
    Some((stem_rank, extension_rank))
}

fn artist_cover_dirs(path: &Path, artist_name: &str) -> Vec<(PathBuf, bool)> {
    let Some(track_dir) = path.parent() else {
        return Vec::new();
    };
    let mut dirs = vec![(track_dir.to_path_buf(), false)];
    if let Some(artist_dir) = track_dir
        .parent()
        .filter(|dir| directory_name_matches_artist(dir, artist_name))
        && artist_dir != track_dir
    {
        dirs.push((artist_dir.to_path_buf(), true));
    }
    dirs
}

fn artist_cover_in_dir(
    dir: &Path,
    artist_name: &str,
    allow_single_fallback: bool,
) -> Option<PathBuf> {
    let image_paths = folder_image_paths(dir);
    image_paths
        .iter()
        .filter_map(|path| {
            explicit_artist_cover_rank(path, artist_name, allow_single_fallback)
                .map(|rank| (rank, path.clone()))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, path)| path)
        .or_else(|| match (allow_single_fallback, image_paths.as_slice()) {
            (true, [path]) => Some(path.clone()),
            _ => None,
        })
}

fn explicit_artist_cover_rank(
    path: &Path,
    artist_name: &str,
    allow_folder_name: bool,
) -> Option<(usize, usize)> {
    let stem = path.file_stem().and_then(|stem| stem.to_str())?;
    let extension = path.extension().and_then(|extension| extension.to_str())?;
    let normalized_stem = normalized_artwork_key(stem);
    let normalized_artist = normalized_artwork_key(artist_name);
    let stem_rank = if normalized_stem == normalized_artist {
        Some(0)
    } else if stem.eq_ignore_ascii_case("artist") {
        Some(1)
    } else if stem.eq_ignore_ascii_case("portrait") {
        Some(2)
    } else if stem.eq_ignore_ascii_case("photo") {
        Some(3)
    } else if allow_folder_name && stem.eq_ignore_ascii_case("folder") {
        Some(4)
    } else {
        None
    }?;
    let extension_rank = ["jpg", "jpeg", "png", "webp"]
        .iter()
        .position(|candidate| extension.eq_ignore_ascii_case(candidate))?;
    Some((stem_rank, extension_rank))
}

fn directory_name_matches_artist(dir: &Path, artist_name: &str) -> bool {
    dir.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| normalized_artwork_key(name) == normalized_artwork_key(artist_name))
}

fn normalized_artwork_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
pub(super) fn embedded_cover(
    path: &Path,
    tagged_file: Option<&lofty::file::TaggedFile>,
    tag: Option<&Tag>,
) -> Option<LocalCover> {
    let _picture = tag
        .and_then(|tag| select_best_picture(tag.pictures()))
        .or_else(|| tagged_file.and_then(|file| select_best_picture_from_tags(file.tags())))?;
    Some(LocalCover::Embedded {
        path: path.to_path_buf(),
        bytes: Arc::<[u8]>::from([]),
        content_type: None,
        revision: Some(file_revision(path).unwrap_or_else(|| path_revision_fallback(path))),
    })
}
pub(super) fn embedded_cover_from_path(path: &Path) -> Option<LocalCover> {
    let tagged_file = Probe::open(path).and_then(|probe| probe.read()).ok()?;
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());
    embedded_cover(path, Some(&tagged_file), tag)
}
pub(super) fn select_best_picture(pictures: &[Picture]) -> Option<&Picture> {
    pictures
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
}
pub(super) fn select_best_picture_from_tags(tags: &[Tag]) -> Option<&Picture> {
    tags.iter()
        .find_map(|tag| select_best_picture(tag.pictures()))
}
pub(super) fn cover_id(cover: &LocalCover) -> String {
    let raw = match cover {
        LocalCover::File { path, .. } => format!("file:{}", path.to_string_lossy()),
        LocalCover::Embedded { path, .. } => format!("embedded:{}", path.to_string_lossy()),
    };
    format!(
        "local:cover:{}",
        utf8_percent_encode(&raw, NON_ALPHANUMERIC)
    )
}
pub(super) fn cover_revision(cover: &LocalCover) -> Option<String> {
    match cover {
        LocalCover::File { revision, .. } | LocalCover::Embedded { revision, .. } => {
            revision.clone()
        }
    }
}
pub(super) fn cover_url(cover: &LocalCover) -> ProviderResult<String> {
    match cover {
        LocalCover::File { path, .. } | LocalCover::Embedded { path, .. } => {
            Url::from_file_path(path)
                .map(|url| url.to_string())
                .map_err(|()| {
                    ProviderError::Other("could not turn cover path into a file URI".to_string())
                })
        }
    }
}
pub(super) fn content_type_from_path(path: &Path) -> Option<String> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => Some("image/jpeg".to_string()),
        Some("png") => Some("image/png".to_string()),
        Some("webp") => Some("image/webp".to_string()),
        _ => None,
    }
}
pub(super) fn tag_string(
    tag: Option<&Tag>,
    read: impl FnOnce(&Tag) -> Option<String>,
) -> Option<String> {
    tag.and_then(read).filter(|value| !value.trim().is_empty())
}
pub(super) fn artist_names(tag: Option<&Tag>, fallback: &str) -> Vec<String> {
    let tagged = tag
        .map(|tag| {
            tag.get_strings(ItemKey::TrackArtists)
                .flat_map(split_credit_names)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let fallback = split_credit_names(fallback);
    if tagged.is_empty() {
        return fallback;
    }
    if tagged.len() == 1 && fallback.len() == 1 && tagged[0].eq_ignore_ascii_case(&fallback[0]) {
        return fallback;
    }
    tagged
}
pub(super) fn split_credit_names(value: &str) -> Vec<String> {
    let names = value
        .split([';', '/'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if names.is_empty() { Vec::new() } else { names }
}
pub(super) fn album_grouping_path(path: &Path) -> String {
    path.parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}
pub(super) fn local_file_cover(path: PathBuf) -> LocalCover {
    LocalCover::File {
        revision: file_revision(&path),
        path,
    }
}
pub(super) fn manifest_cover_from_local(cover: &LocalCover) -> Option<LocalManifestCover> {
    match cover {
        LocalCover::File { path, revision } => Some(LocalManifestCover {
            item_id: cover_id(cover),
            kind: LocalManifestCoverKind::File,
            source_path: path.clone(),
            revision: revision
                .clone()
                .unwrap_or_else(|| path_revision_fallback(path)),
            embedded_index: None,
        }),
        LocalCover::Embedded {
            path,
            content_type: _,
            revision,
            ..
        } => Some(LocalManifestCover {
            item_id: cover_id(cover),
            kind: LocalManifestCoverKind::Embedded,
            source_path: path.clone(),
            revision: revision
                .clone()
                .unwrap_or_else(|| path_revision_fallback(path)),
            embedded_index: None,
        }),
    }
}
pub(super) fn local_cover_from_manifest(cover: &LocalManifestCover) -> LocalCover {
    match cover.kind {
        LocalManifestCoverKind::File => LocalCover::File {
            path: cover.source_path.clone(),
            revision: Some(cover.revision.clone()),
        },
        LocalManifestCoverKind::Embedded => LocalCover::Embedded {
            path: cover.source_path.clone(),
            bytes: Arc::<[u8]>::from([]),
            content_type: None,
            revision: Some(cover.revision.clone()),
        },
    }
}
pub(super) fn merge_genres(target: &mut Vec<String>, source: &[String]) {
    for genre in source {
        if !target.iter().any(|candidate| candidate == genre) {
            target.push(genre.clone());
        }
    }
}
pub(super) fn local_id<T>(kind: &str, value: &str) -> T
where
    T: From<String>,
{
    T::from(format!("local:{kind}:{:016x}", stable_hash(value)))
}
pub(super) fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
pub(super) fn hash_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for part in parts {
        for byte in part.as_bytes().iter().chain(std::iter::once(&0)) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}
pub(super) fn track_metadata_hash(
    track: &Track,
    album_artist: &str,
    musicbrainz_album_id: Option<&str>,
    musicbrainz_release_group_id: Option<&str>,
) -> String {
    let artist_id = track.artist_id.as_ref().map(ArtistId::as_str).unwrap_or("");
    let artist_credits = artist_credit_hash_value(&track.artist_credits);
    let album_artist_credits = artist_credit_hash_value(&track.album_artist_credits);
    let genres = track.genres.join("\u{1f}");
    let year = track.year.to_string();
    let duration_seconds = track.duration_seconds.to_string();
    let disc_number = track.disc_number.to_string();
    let track_number = track.track_number.to_string();
    hash_parts(vec![
        track.title.as_str(),
        track.artist.as_str(),
        artist_id,
        track.album.as_str(),
        album_artist,
        artist_credits.as_str(),
        album_artist_credits.as_str(),
        genres.as_str(),
        track.album_id.as_str(),
        year.as_str(),
        duration_seconds.as_str(),
        disc_number.as_str(),
        track_number.as_str(),
        track.local_path.as_deref().unwrap_or(""),
        track.source_format.as_deref().unwrap_or(""),
        track.comment.as_deref().unwrap_or(""),
        track.musicbrainz_recording_id.as_deref().unwrap_or(""),
        track.musicbrainz_release_track_id.as_deref().unwrap_or(""),
        musicbrainz_album_id.unwrap_or(""),
        musicbrainz_release_group_id.unwrap_or(""),
    ])
}
pub(super) fn track_search_hash(track: &Track) -> String {
    let artist_credits = artist_credit_hash_value(&track.artist_credits);
    let album_artist_credits = artist_credit_hash_value(&track.album_artist_credits);
    let genres = track.genres.join("\u{1f}");
    hash_parts(vec![
        track.title.as_str(),
        track.album_id.as_str(),
        track.album.as_str(),
        track.artist.as_str(),
        artist_credits.as_str(),
        album_artist_credits.as_str(),
        genres.as_str(),
    ])
}
fn artist_credit_hash_value(credits: &[ArtistCredit]) -> String {
    credits
        .iter()
        .map(|credit| {
            format!(
                "{}:{}:{}",
                credit.id.as_str(),
                credit.name,
                credit.musicbrainz_artist_id.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\u{1f}")
}
pub(super) fn file_revision(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(format!(
        "file:{:016x}",
        stable_hash(&format!(
            "{}:{}:{}:{}",
            path.to_string_lossy(),
            metadata.len(),
            duration.as_secs(),
            duration.subsec_nanos()
        ))
    ))
}
fn path_revision_fallback(path: &Path) -> String {
    format!("path:{:016x}", stable_hash(&path.to_string_lossy()))
}
pub(super) fn normalize_search(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub(super) fn searchable_matches<'a>(
    query: &str,
    mut values: impl Iterator<Item = &'a String>,
) -> bool {
    values.any(|value| normalize_search(value).contains(query))
}
#[allow(dead_code)]
pub(super) fn decode_cover_id(item_id: &str) -> Option<String> {
    item_id
        .strip_prefix("local:cover:")
        .and_then(|encoded| percent_decode_str(encoded).decode_utf8().ok())
        .map(|decoded| decoded.into_owned())
}
