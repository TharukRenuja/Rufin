use std::{cell::Cell, rc::Rc};

use adw::prelude::*;
use artwork::ArtworkBinding;

use super::{ArtworkTile, GRID_COVER_SIZE};
use crate::shell::Shell;

#[derive(Clone)]
pub(crate) struct CoverGroupProjection {
    root: gtk::Stack,
    single: ArtworkTile,
    grid: gtk::Grid,
    quadrants: Rc<Vec<ArtworkTile>>,
    size: Rc<Cell<i32>>,
    fetch_size: u32,
}

impl CoverGroupProjection {
    pub(crate) fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub(crate) fn replace(&self, shell: &Rc<Shell>, artwork: &[ArtworkBinding], seed: u32) {
        let size = self.size.get();
        if artwork.len() <= 1 {
            for tile in self.quadrants.iter() {
                shell.clear_artwork_tile(tile);
            }
            shell.bind_artwork_tile(
                &self.single,
                artwork.first().cloned().unwrap_or_else(ArtworkBinding::new),
                seed,
                size,
                self.fetch_size,
            );
            self.root.set_visible_child_name("single");
            return;
        }

        shell.clear_artwork_tile(&self.single);
        let cell_size = (size / 2).max(1);
        for (index, tile) in self.quadrants.iter().enumerate() {
            shell.bind_artwork_tile(
                tile,
                artwork[index % artwork.len()].clone(),
                seed.wrapping_add((index as u32).wrapping_mul(0x9e37_79b9)),
                cell_size,
                self.fetch_size,
            );
        }
        self.root.set_visible_child_name("grid");
    }

    pub(crate) fn resize(&self, size: i32) {
        let size = size.max(1);
        if self.size.replace(size) == size {
            return;
        }
        self.root.set_size_request(size, size);
        self.grid.set_size_request(size, size);
        self.single.set_square_size(size);
        let cell_size = (size / 2).max(1);
        for tile in self.quadrants.iter() {
            tile.set_square_size(cell_size);
        }
    }
}

impl Shell {
    pub(crate) fn cover_group_projection_for_artwork(
        self: &Rc<Self>,
        artwork: &[ArtworkBinding],
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> CoverGroupProjection {
        let root = gtk::Stack::new();
        root.set_size_request(size, size);
        root.set_hexpand(false);
        root.set_vexpand(false);
        root.set_halign(gtk::Align::Start);
        root.set_valign(gtk::Align::Start);

        let single = ArtworkTile::new_sized(size, size, seed);
        root.add_named(&single.widget(), Some("single"));

        let grid = gtk::Grid::new();
        grid.add_css_class("cover-tile");
        grid.add_css_class("card");
        grid.set_size_request(size, size);
        grid.set_overflow(gtk::Overflow::Hidden);
        grid.set_row_homogeneous(true);
        grid.set_column_homogeneous(true);
        let cell_size = (size / 2).max(1);
        let quadrants = Rc::new(
            (0..4)
                .map(|index| {
                    let tile = ArtworkTile::new_sized(
                        cell_size,
                        cell_size,
                        seed.wrapping_add((index as u32).wrapping_mul(0x9e37_79b9)),
                    );
                    grid.attach(&tile.widget(), (index % 2) as i32, (index / 2) as i32, 1, 1);
                    tile
                })
                .collect::<Vec<_>>(),
        );
        root.add_named(&grid, Some("grid"));

        let projection = CoverGroupProjection {
            root,
            single,
            grid,
            quadrants,
            size: Rc::new(Cell::new(size)),
            fetch_size,
        };
        projection.replace(self, artwork, seed);
        projection
    }

    pub(crate) fn elastic_cover_tile_for_candidates(
        self: &Rc<Self>,
        candidates: ArtworkBinding,
        seed: u32,
        fetch_size: u32,
    ) -> (gtk::Widget, ArtworkTile) {
        let tile = ArtworkTile::new_elastic_square(seed);
        let widget = tile.widget();
        self.bind_artwork_tile(&tile, candidates, seed, GRID_COVER_SIZE as i32, fetch_size);
        (widget, tile)
    }

    pub(crate) fn cover_tile_for_candidates(
        self: &Rc<Self>,
        candidates: ArtworkBinding,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        self.cover_tile_for_candidate_dimensions(candidates, seed, size, size, fetch_size)
    }

    pub(crate) fn cover_tile_for_candidate_dimensions(
        self: &Rc<Self>,
        candidates: ArtworkBinding,
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

    pub(crate) fn elastic_cover_group_tile_for_artwork(
        self: &Rc<Self>,
        artwork: &[ArtworkBinding],
        seed: u32,
        fetch_size: u32,
    ) -> gtk::Widget {
        match artwork.len() {
            0 => {
                self.elastic_cover_tile_for_candidates(ArtworkBinding::new(), seed, fetch_size)
                    .0
            }
            1 => {
                self.elastic_cover_tile_for_candidates(artwork[0].clone(), seed, fetch_size)
                    .0
            }
            _ => {
                let grid = gtk::Grid::new();
                grid.add_css_class("cover-tile");
                grid.add_css_class("card");
                grid.set_overflow(gtk::Overflow::Hidden);
                grid.set_row_homogeneous(true);
                grid.set_column_homogeneous(true);
                grid.set_hexpand(true);
                grid.set_vexpand(true);
                grid.set_halign(gtk::Align::Fill);
                grid.set_valign(gtk::Align::Fill);

                for index in 0..4 {
                    let child = self
                        .elastic_cover_tile_for_candidates(
                            artwork[index % artwork.len()].clone(),
                            seed.wrapping_add((index as u32).wrapping_mul(0x9e37_79b9)),
                            fetch_size,
                        )
                        .0;
                    grid.attach(&child, (index % 2) as i32, (index / 2) as i32, 1, 1);
                }
                grid.upcast()
            }
        }
    }
}
