mod bottom;
pub(crate) mod desktop;
mod equalizer;
pub(crate) mod fullscreen;
mod icons;
pub(crate) mod lyrics;
mod outputs;
mod progress;
pub(crate) mod queue;
mod random_play;
pub(crate) mod right_panel;
pub(crate) mod state;

pub(crate) use bottom::{PlayerControls, build_bottom_player, connect_player_controls};
pub(crate) use desktop::{install_tray, present_initial_window};
pub(crate) use desktop::{
    now_playing_notification_can_send, now_playing_notification_should_withdraw,
};
pub(crate) use equalizer::{
    build_equalizer_preset_row, connect_equalizer_scale_commit, equalizer_band_title,
    equalizer_default_preset_bands, equalizer_preset_bands, equalizer_preset_name_at,
    equalizer_preset_position, equalizer_selected_preset, install_equalizer_scroll,
};
pub(crate) use fullscreen::{
    FullscreenPlayerParts, build_fullscreen_player, connect_fullscreen_player_controls,
};
pub(crate) use outputs::{
    default_audio_output_options, present_audio_output_popover, selected_audio_output_title,
    warm_audio_output_cache,
};
pub(crate) use queue::connect_queue_panel_controls;
pub(crate) use right_panel::{
    apply_lyrics_panel_visibility, build_right_panel, connect_queue_lyrics_overlay,
};

pub(crate) struct PlayerDesktopWidgets {
    pub(crate) fullscreen_player: FullscreenPlayerParts,
    pub(crate) player_controls: PlayerControls,
}
