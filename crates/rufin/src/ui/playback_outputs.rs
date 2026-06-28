use super::*;
use playback::available_audio_outputs;

pub(in crate::ui) fn playback_output_options() -> Vec<(Option<String>, String)> {
    let mut outputs = vec![(None, tr("System default"))];
    outputs.extend(
        available_audio_outputs()
            .into_iter()
            .filter(|output| output.id != "autoaudiosink")
            .map(|output| (Some(output.id), output.name)),
    );
    outputs
}

pub(in crate::ui) fn audio_output_index(
    outputs: &[(Option<String>, String)],
    selected: Option<&str>,
) -> u32 {
    outputs
        .iter()
        .position(|(id, _)| id.as_deref() == selected)
        .unwrap_or_default() as u32
}
