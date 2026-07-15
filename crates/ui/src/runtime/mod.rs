//! Commands, updates, and startup data exchanged between the UI and Rufin.
//!
//! `rufin` constructs these handles; the crates behind them implement the behavior.

pub mod artwork;
mod events;
mod inputs;
pub mod library;
pub mod lyrics;
pub mod source;

pub use ::playback::PlaybackHandles;
pub use artwork::{ArtworkHandle, ArtworkPort};
pub use events::ProductReceivers;
pub use inputs::RuntimeInputs;
pub use library::{LibraryHandle, LibraryPort};
pub use lyrics::{LyricsHandle, LyricsPort};
pub use source::{SourceHandle, SourcePort};

#[derive(Clone)]
pub struct ProductHandles {
    pub source: SourceHandle,
    pub library: LibraryHandle,
    pub playback: PlaybackHandles,
    pub artwork: ArtworkHandle,
    pub lyrics: LyricsHandle,
}
