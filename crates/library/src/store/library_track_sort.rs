use super::*;

pub(super) fn track_order_by_sql(alias: &str, field: TrackSort, descending: bool) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    let expression = match field {
        TrackSort::TrackNumber => {
            return format!(
                "{alias}.disc_number {direction}, {alias}.track_number {direction}, {}",
                track_tiebreaker_order_sql(alias, direction)
            );
        }
        TrackSort::Artist => format!("{alias}.artist COLLATE NOCASE"),
        TrackSort::AlbumArtist => format!(
            "COALESCE((SELECT aal.name FROM album_artist_links aal WHERE aal.source_id = {alias}.source_id AND aal.album_id = {alias}.album_id ORDER BY aal.position LIMIT 1), {alias}.artist) COLLATE NOCASE"
        ),
        TrackSort::Album => format!("{alias}.album COLLATE NOCASE"),
        TrackSort::Year => format!("{alias}.year"),
        TrackSort::ReleaseDate => format!("{alias}.release_date"),
        TrackSort::DateAdded => format!("{alias}.date_added"),
        TrackSort::LastPlayed => format!("{alias}.last_played"),
        TrackSort::PlayCount => format!("{alias}.play_count"),
        TrackSort::UserRating => format!("{alias}.user_rating"),
        TrackSort::Genre => format!(
            "(SELECT tg.genre_name FROM track_genres tg WHERE tg.source_id = {alias}.source_id AND tg.track_id = {alias}.track_id ORDER BY tg.genre_name COLLATE NOCASE LIMIT 1) COLLATE NOCASE"
        ),
        TrackSort::Bpm => format!("{alias}.bpm"),
        TrackSort::Duration => format!("{alias}.duration_seconds"),
        TrackSort::Favorite => effective_track_favorite_sql(alias),
        TrackSort::Title => format!("{alias}.title COLLATE NOCASE"),
    };
    let missing = match field {
        TrackSort::ReleaseDate
        | TrackSort::DateAdded
        | TrackSort::LastPlayed
        | TrackSort::PlayCount
        | TrackSort::UserRating
        | TrackSort::Bpm => format!("{expression} IS NULL ASC, "),
        TrackSort::Title
        | TrackSort::Artist
        | TrackSort::AlbumArtist
        | TrackSort::Album
        | TrackSort::Year
        | TrackSort::Genre
        | TrackSort::TrackNumber
        | TrackSort::Duration
        | TrackSort::Favorite => String::new(),
    };
    format!(
        "{missing}{expression} {direction}, {}",
        track_tiebreaker_order_sql(alias, direction)
    )
}

fn track_tiebreaker_order_sql(alias: &str, direction: &str) -> String {
    format!(
        "{alias}.album COLLATE NOCASE {direction}, {alias}.disc_number {direction}, {alias}.track_number {direction}, {alias}.title COLLATE NOCASE {direction}, {alias}.track_id {direction}"
    )
}
