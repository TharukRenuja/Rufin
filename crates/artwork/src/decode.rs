use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gdk_pixbuf::{InterpType, Pixbuf};

use crate::{ArtworkError, ArtworkKey};

pub(crate) struct NormalizedImage {
    pixbuf: Pixbuf,
    bytes: Vec<u8>,
}

impl NormalizedImage {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug)]
pub struct DecodedImage {
    key: ArtworkKey,
    cache_path: Arc<PathBuf>,
    width: u32,
    height: u32,
    row_stride: u32,
    rgba: Arc<[u8]>,
}

impl DecodedImage {
    pub(crate) fn key(&self) -> &ArtworkKey {
        &self.key
    }

    pub fn cache_path(&self) -> &Path {
        self.cache_path.as_ref()
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn row_stride(&self) -> u32 {
        self.row_stride
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

pub(crate) fn decode_cached(
    path: PathBuf,
    key: ArtworkKey,
    render_size: u32,
) -> Result<DecodedImage, ArtworkError> {
    let pixbuf =
        Pixbuf::from_file(&path).map_err(|error| ArtworkError::Decode(error.to_string()))?;
    decode_pixbuf(pixbuf, path, key, render_size)
}

pub(crate) fn decode_normalized(
    image: NormalizedImage,
    path: PathBuf,
    key: ArtworkKey,
    render_size: u32,
) -> Result<DecodedImage, ArtworkError> {
    decode_pixbuf(image.pixbuf, path, key, render_size)
}

fn decode_pixbuf(
    pixbuf: Pixbuf,
    path: PathBuf,
    key: ArtworkKey,
    render_size: u32,
) -> Result<DecodedImage, ArtworkError> {
    let pixbuf = scale_to_fit(&pixbuf, render_size.max(1))?;
    let width = u32::try_from(pixbuf.width())
        .map_err(|_| ArtworkError::Decode("artwork width was invalid".to_string()))?;
    let height = u32::try_from(pixbuf.height())
        .map_err(|_| ArtworkError::Decode("artwork height was invalid".to_string()))?;
    let source_stride = usize::try_from(pixbuf.rowstride())
        .map_err(|_| ArtworkError::Decode("artwork row stride was invalid".to_string()))?;
    let channels = usize::try_from(pixbuf.n_channels())
        .map_err(|_| ArtworkError::Decode("artwork channel count was invalid".to_string()))?;
    if !(channels == 3 || channels == 4) {
        return Err(ArtworkError::Decode(format!(
            "unsupported artwork channel count {channels}"
        )));
    }
    let pixels = pixbuf.read_pixel_bytes();
    let source = pixels.as_ref();
    let width_usize = width as usize;
    let height_usize = height as usize;
    let row_stride = width
        .checked_mul(4)
        .ok_or_else(|| ArtworkError::Decode("artwork dimensions overflowed".to_string()))?;
    let mut rgba = vec![0; row_stride as usize * height_usize];
    for y in 0..height_usize {
        let source_row = y
            .checked_mul(source_stride)
            .ok_or_else(|| ArtworkError::Decode("artwork row offset overflowed".to_string()))?;
        for x in 0..width_usize {
            let source_offset = source_row.checked_add(x * channels).ok_or_else(|| {
                ArtworkError::Decode("artwork pixel offset overflowed".to_string())
            })?;
            let destination_offset = (y * width_usize + x) * 4;
            let Some(rgb) = source.get(source_offset..source_offset + 3) else {
                return Err(ArtworkError::Decode(
                    "artwork pixel data was truncated".to_string(),
                ));
            };
            rgba[destination_offset..destination_offset + 3].copy_from_slice(rgb);
            rgba[destination_offset + 3] = if channels == 4 {
                source.get(source_offset + 3).copied().ok_or_else(|| {
                    ArtworkError::Decode("artwork alpha data was truncated".to_string())
                })?
            } else {
                u8::MAX
            };
        }
    }
    Ok(DecodedImage {
        key,
        cache_path: Arc::new(path),
        width,
        height,
        row_stride,
        rgba: rgba.into(),
    })
}

pub(crate) fn normalize_for_cache(
    bytes: Vec<u8>,
    size: u32,
) -> Result<NormalizedImage, ArtworkError> {
    let pixbuf = Pixbuf::from_read(Cursor::new(bytes))
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    let pixbuf = scale_to_fit(&pixbuf, size.max(1))?;
    let bytes = pixbuf
        .save_to_bufferv("png", &[])
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    Ok(NormalizedImage { pixbuf, bytes })
}

fn scale_to_fit(pixbuf: &Pixbuf, target: u32) -> Result<Pixbuf, ArtworkError> {
    let width = u32::try_from(pixbuf.width())
        .map_err(|_| ArtworkError::Decode("artwork width was invalid".to_string()))?;
    let height = u32::try_from(pixbuf.height())
        .map_err(|_| ArtworkError::Decode("artwork height was invalid".to_string()))?;
    if width == 0 || height == 0 {
        return Err(ArtworkError::Decode(
            "artwork dimensions were empty".to_string(),
        ));
    }
    let longest = width.max(height);
    if longest <= target {
        return Ok(pixbuf.clone());
    }
    let scaled_width = (u64::from(width) * u64::from(target) / u64::from(longest)).max(1);
    let scaled_height = (u64::from(height) * u64::from(target) / u64::from(longest)).max(1);
    let scaled_width = i32::try_from(scaled_width)
        .map_err(|_| ArtworkError::Decode("scaled artwork width was invalid".to_string()))?;
    let scaled_height = i32::try_from(scaled_height)
        .map_err(|_| ArtworkError::Decode("scaled artwork height was invalid".to_string()))?;
    pixbuf
        .scale_simple(scaled_width, scaled_height, InterpType::Bilinear)
        .ok_or_else(|| ArtworkError::Decode("artwork could not be scaled".to_string()))
}
