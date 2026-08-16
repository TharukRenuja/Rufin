use library::{
    Libraries, LyricsCacheAuthority, LyricsCacheInput, LyricsCacheKey, LyricsCacheWrite, SourceId,
    TrackId,
};

#[test]
fn lyrics_cache_reopens_as_an_opaque_bounded_value() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let path = directory.path().join("library.db");
    let key = LyricsCacheKey {
        source_id: SourceId::new("local:lyrics-cache"),
        track_id: TrackId::new("track:lyrics-cache"),
        role: "future-display-role".to_string(),
        language: "ja".to_string(),
        script: "Jpan".to_string(),
    };
    let input = LyricsCacheInput { digest: [7; 32] };
    let payload = "Lyrics owns the representation; Library preserves these bytes.".to_string();

    {
        let library = Libraries::open(&path).expect("open Library");
        let trim = library
            .store_lyrics(LyricsCacheWrite {
                key: key.clone(),
                authority: LyricsCacheAuthority::External,
                input: input.clone(),
                payload: payload.clone(),
                cached_at: 42,
            })
            .expect("store lyrics cache");
        assert_eq!(trim.rows_removed, 0);
        assert_eq!(trim.bytes_removed, 0);
    }

    let library = Libraries::open(&path).expect("reopen Library");
    let cached = library
        .cached_lyrics(key.clone(), input.clone())
        .expect("read lyrics cache")
        .expect("stored lyrics cache");
    assert_eq!(cached.key, key);
    assert_eq!(cached.authority, LyricsCacheAuthority::External);
    assert_eq!(cached.input, input);
    assert_eq!(cached.payload, payload);
    assert_eq!(cached.cached_at, 42);

    assert!(
        !library
            .remove_lyrics_if_authority(key.clone(), LyricsCacheAuthority::Source)
            .expect("keep cache owned by another authority")
    );
    assert!(
        library
            .remove_lyrics_if_authority(key.clone(), LyricsCacheAuthority::External)
            .expect("remove matching cache authority")
    );
    assert!(
        library
            .cached_lyrics(key, input)
            .expect("read removed lyrics cache")
            .is_none()
    );
}

#[test]
fn incompatible_lyrics_input_discards_the_rebuildable_row() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let library =
        Libraries::open(directory.path().join("library.db")).expect("open temporary Library");
    let key = LyricsCacheKey {
        source_id: SourceId::new("jellyfin:lyrics-cache"),
        track_id: TrackId::new("track:lyrics-cache"),
        role: "original".to_string(),
        language: String::new(),
        script: String::new(),
    };
    let stored_input = LyricsCacheInput { digest: [1; 32] };
    library
        .store_lyrics(LyricsCacheWrite {
            key: key.clone(),
            authority: LyricsCacheAuthority::Source,
            input: stored_input.clone(),
            payload: "{}".to_string(),
            cached_at: 1,
        })
        .expect("store lyrics cache");

    assert!(
        library
            .cached_lyrics(key.clone(), LyricsCacheInput { digest: [2; 32] },)
            .expect("reject incompatible cache input")
            .is_none()
    );
    assert!(
        library
            .cached_lyrics(key, stored_input)
            .expect("read discarded cache row")
            .is_none()
    );
}

#[test]
fn clearing_fetched_track_lyrics_removes_every_language_variant_only() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let library =
        Libraries::open(directory.path().join("library.db")).expect("open temporary Library");
    let source_id = SourceId::new("subsonic:lyrics-cache");
    let track_id = TrackId::new("track:variants");
    let input = LyricsCacheInput { digest: [3; 32] };
    for (role, language, authority) in [
        ("original", "", LyricsCacheAuthority::External),
        ("translation", "en", LyricsCacheAuthority::External),
        ("translation", "fr", LyricsCacheAuthority::Source),
    ] {
        library
            .store_lyrics(LyricsCacheWrite {
                key: LyricsCacheKey {
                    source_id: source_id.clone(),
                    track_id: track_id.clone(),
                    role: role.to_string(),
                    language: language.to_string(),
                    script: String::new(),
                },
                authority,
                input: input.clone(),
                payload: "{}".to_string(),
                cached_at: 1,
            })
            .expect("store lyrics variant");
    }

    assert_eq!(
        library
            .remove_track_lyrics_by_authority(
                source_id.clone(),
                track_id.clone(),
                LyricsCacheAuthority::External,
            )
            .expect("clear fetched variants"),
        2
    );
    assert!(
        library
            .cached_lyrics(
                LyricsCacheKey {
                    source_id,
                    track_id,
                    role: "translation".to_string(),
                    language: "fr".to_string(),
                    script: String::new(),
                },
                input,
            )
            .expect("read source variant")
            .is_some()
    );
}
