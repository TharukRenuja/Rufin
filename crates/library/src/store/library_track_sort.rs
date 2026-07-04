use super::*;

pub(super) fn track_order_by_sql(alias: &str, field: LibraryField, descending: bool) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    let expression = match field {
        LibraryField::TrackNumber => {
            return format!(
                "{alias}.disc_number {direction}, {alias}.track_number {direction}, {}",
                track_tiebreaker_order_sql(alias, direction)
            );
        }
        LibraryField::Artist => format!("{alias}.artist COLLATE NOCASE"),
        LibraryField::AlbumArtist => format!(
            "COALESCE((SELECT aal.name FROM album_artist_links aal WHERE aal.server_id = {alias}.server_id AND aal.album_id = {alias}.album_id ORDER BY aal.position LIMIT 1), {alias}.artist) COLLATE NOCASE"
        ),
        LibraryField::Album => format!("{alias}.album COLLATE NOCASE"),
        LibraryField::Year => format!("{alias}.year"),
        LibraryField::ReleaseDate => format!("{alias}.release_date"),
        LibraryField::DateAdded => format!("{alias}.date_added"),
        LibraryField::LastPlayed => format!("{alias}.last_played"),
        LibraryField::PlayCount => format!("{alias}.play_count"),
        LibraryField::UserRating => format!("{alias}.user_rating"),
        LibraryField::Genre => format!(
            "(SELECT tg.genre_name FROM track_genres tg WHERE tg.server_id = {alias}.server_id AND tg.track_id = {alias}.track_id ORDER BY tg.genre_name COLLATE NOCASE LIMIT 1) COLLATE NOCASE"
        ),
        LibraryField::Duration => format!("{alias}.duration_seconds"),
        LibraryField::Favorite => effective_track_favorite_sql(alias),
        LibraryField::RowIndex
        | LibraryField::Image
        | LibraryField::Title
        | LibraryField::TitleMerged
        | LibraryField::DiscNumber
        | LibraryField::SongCount
        | LibraryField::AlbumCount => format!("{alias}.title COLLATE NOCASE"),
    };
    let missing = match field {
        LibraryField::ReleaseDate
        | LibraryField::DateAdded
        | LibraryField::LastPlayed
        | LibraryField::PlayCount
        | LibraryField::UserRating => format!("{expression} IS NULL ASC, "),
        LibraryField::RowIndex
        | LibraryField::Image
        | LibraryField::Title
        | LibraryField::TitleMerged
        | LibraryField::Artist
        | LibraryField::AlbumArtist
        | LibraryField::Album
        | LibraryField::Year
        | LibraryField::Genre
        | LibraryField::TrackNumber
        | LibraryField::DiscNumber
        | LibraryField::SongCount
        | LibraryField::AlbumCount
        | LibraryField::Duration
        | LibraryField::Favorite => String::new(),
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
