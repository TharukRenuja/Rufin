mod cover;
mod http;
mod musicbrainz;

pub use cover::{AlbumCover, AlbumCoverPolicy, lookup_album_cover, public_album_cover_url};
pub use musicbrainz::{AlbumReleaseMetadata, identify_metadata, lookup_album_release};
