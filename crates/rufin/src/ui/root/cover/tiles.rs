use super::*;

impl Shell {
    pub(in crate::ui) fn cover_tile_for_candidates(
        self: &Rc<Self>,
        candidates: CandidateSet,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        self.cover_tile_for_candidate_dimensions(candidates, seed, size, size, fetch_size)
    }

    pub(in crate::ui) fn cover_tile_for_candidate_dimensions(
        self: &Rc<Self>,
        candidates: CandidateSet,
        seed: u32,
        width: i32,
        height: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let tile = ArtworkTile::new_sized(width, height, seed);
        let widget = tile.widget();
        self.bind_artwork_tile(&tile, candidates, seed, width.max(height), fetch_size);
        widget
    }

    pub(in crate::ui) fn cover_group_tile_for_artwork(
        self: &Rc<Self>,
        artwork: &[CandidateSet],
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        match artwork.len() {
            0 => self.cover_tile_for_candidates(CandidateSet::new(), seed, size, fetch_size),
            1 => self.cover_tile_for_candidates(artwork[0].clone(), seed, size, fetch_size),
            _ => {
                let grid = gtk::Grid::new();
                grid.add_css_class("cover-tile");
                grid.add_css_class("card");
                grid.set_size_request(size, size);
                grid.set_width_request(size);
                grid.set_height_request(size);
                grid.set_overflow(gtk::Overflow::Hidden);
                grid.set_row_homogeneous(true);
                grid.set_column_homogeneous(true);
                grid.set_hexpand(false);
                grid.set_vexpand(false);
                grid.set_halign(gtk::Align::Start);
                grid.set_valign(gtk::Align::Start);

                let cell_size = (size / 2).max(1);
                for index in 0..4 {
                    let child = self.cover_tile_for_candidates(
                        artwork[index % artwork.len()].clone(),
                        seed.wrapping_add((index as u32).wrapping_mul(0x9e37_79b9)),
                        cell_size,
                        fetch_size,
                    );
                    grid.attach(&child, (index % 2) as i32, (index / 2) as i32, 1, 1);
                }
                grid.upcast()
            }
        }
    }
}
