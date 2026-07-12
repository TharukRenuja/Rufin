use super::*;

pub(in crate::ui) fn notification_icon_path(path: &Path) -> Option<Vec<u8>> {
    let pixbuf = Pixbuf::from_file(path).ok()?;
    notification_icon_pixbuf(&pixbuf)
}
pub(in crate::ui) fn notification_icon_pixbuf(pixbuf: &Pixbuf) -> Option<Vec<u8>> {
    let target_size = THUMB_COVER_SIZE.clamp(1, 512) as i32;
    let width = pixbuf.width().max(1);
    let height = pixbuf.height().max(1);
    let crop_size = width.min(height);
    let crop_x = (width - crop_size) / 2;
    let crop_y = (height - crop_size) / 2;
    let cropped = Pixbuf::new(Colorspace::Rgb, pixbuf.has_alpha(), 8, crop_size, crop_size)?;
    pixbuf.copy_area(crop_x, crop_y, crop_size, crop_size, &cropped, 0, 0);
    let icon = if crop_size == target_size {
        cropped
    } else {
        cropped.scale_simple(target_size, target_size, InterpType::Bilinear)?
    };

    icon.save_to_bufferv("png", &[]).ok()
}
pub(in crate::ui) fn cover_decode_size(display_size: i32, fetch_size: u32) -> i32 {
    display_size.max(fetch_size as i32).max(1)
}
pub(in crate::ui) fn cover_fetch_size_for_display(display_size: i32) -> u32 {
    if display_size <= THUMB_COVER_SIZE as i32 {
        THUMB_COVER_SIZE
    } else if display_size <= GRID_COVER_SIZE as i32 {
        GRID_COVER_SIZE
    } else {
        DETAIL_COVER_SIZE
    }
}
pub(in crate::ui) fn prefetched_explore_from_snapshot(
    snapshot: &LibrarySnapshot,
) -> Option<PrefetchedHomeSection> {
    Some(PrefetchedHomeSection {
        source_id: snapshot.source.as_ref()?.id.clone(),
        section: snapshot.prefetched_explore.clone()?,
    })
}
pub(in crate::ui) fn upsert_snapshot_home_section(
    sections: &mut Vec<HomeSection>,
    section: HomeSection,
) {
    if let Some(existing) = sections
        .iter_mut()
        .find(|existing| existing.kind == section.kind)
    {
        *existing = section;
    } else if section.kind == HomeSectionKind::Explore {
        sections.insert(0, section);
    } else {
        sections.push(section);
    }
}
