use std::io::{BufRead, Cursor, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, metadata::Orientation};

use crate::{ArtworkError, ArtworkKey, DecodedImageIdentity};

pub(crate) struct NormalizedImage {
    image: DynamicImage,
    bytes: Vec<u8>,
}

impl NormalizedImage {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug)]
pub struct RgbaImage {
    width: u32,
    height: u32,
    row_stride: u32,
    rgba: Arc<[u8]>,
}

impl RgbaImage {
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

    pub fn resized_exact(&self, width: u32, height: u32) -> Result<Self, ArtworkError> {
        let source = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
            self.width,
            self.height,
            self.rgba(),
        )
        .ok_or_else(|| ArtworkError::Decode("artwork pixel data was truncated".to_string()))?;
        let resized =
            image::imageops::resize(&source, width.max(1), height.max(1), FilterType::Lanczos3);
        rgba_buffer(resized)
    }
}

#[derive(Clone, Debug)]
pub struct DecodedImage {
    key: ArtworkKey,
    cache_path: Arc<PathBuf>,
    pixels: RgbaImage,
}

impl DecodedImage {
    pub(crate) fn key(&self) -> &ArtworkKey {
        &self.key
    }

    pub fn identity(&self) -> DecodedImageIdentity {
        DecodedImageIdentity(self.key.clone())
    }

    pub fn cache_path(&self) -> &Path {
        self.cache_path.as_ref()
    }

    pub const fn width(&self) -> u32 {
        self.pixels.width()
    }

    pub const fn height(&self) -> u32 {
        self.pixels.height()
    }

    pub const fn row_stride(&self) -> u32 {
        self.pixels.row_stride()
    }

    pub fn rgba(&self) -> &[u8] {
        self.pixels.rgba()
    }
}

#[cfg(test)]
pub(crate) fn decoded_image_for_test(key: ArtworkKey, bytes: usize) -> DecodedImage {
    assert!(bytes >= 4 && bytes.is_multiple_of(4));
    let width = u32::try_from(bytes / 4).expect("test image width");
    DecodedImage {
        key,
        cache_path: Arc::new(PathBuf::from("test-artwork")),
        pixels: RgbaImage {
            width,
            height: 1,
            row_stride: width * 4,
            rgba: vec![0; bytes].into(),
        },
    }
}

pub fn decode_rgba(bytes: &[u8], render_size: u32) -> Result<RgbaImage, ArtworkError> {
    let image = decode_reader(
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(decode_error)?,
    )?;
    rgba_image(scale_to_fit(image, render_size.max(1))?)
}

pub fn square_thumbnail_png(bytes: &[u8], size: u32) -> Result<Vec<u8>, ArtworkError> {
    let image = decode_reader(
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(decode_error)?,
    )?;
    let target = size.max(1);
    let crop_size = image.width().min(image.height());
    if crop_size == 0 {
        return Err(ArtworkError::Decode(
            "artwork dimensions were empty".to_string(),
        ));
    }
    let crop_x = (image.width() - crop_size) / 2;
    let crop_y = (image.height() - crop_size) / 2;
    let cropped = image.crop_imm(crop_x, crop_y, crop_size, crop_size);
    let thumbnail = if crop_size == target {
        cropped
    } else {
        cropped.resize_exact(target, target, FilterType::Triangle)
    };
    encode_png(&thumbnail)
}

pub(crate) fn decode_cached(
    path: PathBuf,
    key: ArtworkKey,
    render_size: u32,
) -> Result<DecodedImage, ArtworkError> {
    let reader = ImageReader::open(&path)
        .and_then(ImageReader::with_guessed_format)
        .map_err(decode_error)?;
    let image = decode_reader(reader)?;
    decoded_image(image, path, key, render_size)
}

pub(crate) fn decode_normalized(
    image: NormalizedImage,
    path: PathBuf,
    key: ArtworkKey,
    render_size: u32,
) -> Result<DecodedImage, ArtworkError> {
    decoded_image(image.image, path, key, render_size)
}

fn decoded_image(
    image: DynamicImage,
    path: PathBuf,
    key: ArtworkKey,
    render_size: u32,
) -> Result<DecodedImage, ArtworkError> {
    Ok(DecodedImage {
        key,
        cache_path: Arc::new(path),
        pixels: rgba_image(scale_to_fit(image, render_size.max(1))?)?,
    })
}

pub(crate) fn normalize_for_cache(
    bytes: Vec<u8>,
    size: u32,
) -> Result<NormalizedImage, ArtworkError> {
    let image = decode_reader(
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(decode_error)?,
    )?;
    let image = scale_to_fit(image, size.max(1))?;
    let bytes = encode_png(&image)?;
    Ok(NormalizedImage { image, bytes })
}

fn decode_reader<R>(reader: ImageReader<R>) -> Result<DynamicImage, ArtworkError>
where
    R: BufRead + Seek,
{
    let mut decoder = reader.into_decoder().map_err(decode_error)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(decode_error)?;
    image.apply_orientation(orientation);
    if image.color().has_alpha() {
        Ok(DynamicImage::ImageRgba8(image.into_rgba8()))
    } else {
        Ok(DynamicImage::ImageRgb8(image.into_rgb8()))
    }
}

fn scale_to_fit(image: DynamicImage, target: u32) -> Result<DynamicImage, ArtworkError> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err(ArtworkError::Decode(
            "artwork dimensions were empty".to_string(),
        ));
    }
    let longest = width.max(height);
    if longest <= target {
        return Ok(image);
    }
    let scaled_width = (u64::from(width) * u64::from(target) / u64::from(longest)).max(1);
    let scaled_height = (u64::from(height) * u64::from(target) / u64::from(longest)).max(1);
    let scaled_width = u32::try_from(scaled_width)
        .map_err(|_| ArtworkError::Decode("scaled artwork width was invalid".to_string()))?;
    let scaled_height = u32::try_from(scaled_height)
        .map_err(|_| ArtworkError::Decode("scaled artwork height was invalid".to_string()))?;
    Ok(image.resize_exact(scaled_width, scaled_height, FilterType::Triangle))
}

fn rgba_image(image: DynamicImage) -> Result<RgbaImage, ArtworkError> {
    rgba_buffer(image.into_rgba8())
}

fn rgba_buffer(image: image::RgbaImage) -> Result<RgbaImage, ArtworkError> {
    let width = image.width();
    let height = image.height();
    let row_stride = width
        .checked_mul(4)
        .ok_or_else(|| ArtworkError::Decode("artwork dimensions overflowed".to_string()))?;
    let expected = usize::try_from(row_stride)
        .ok()
        .and_then(|stride| stride.checked_mul(height as usize))
        .ok_or_else(|| ArtworkError::Decode("artwork dimensions overflowed".to_string()))?;
    let rgba = image.into_raw();
    if rgba.len() != expected {
        return Err(ArtworkError::Decode(
            "artwork pixel data was truncated".to_string(),
        ));
    }
    Ok(RgbaImage {
        width,
        height,
        row_stride,
        rgba: rgba.into(),
    })
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, ArtworkError> {
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(decode_error)?;
    Ok(bytes.into_inner())
}

fn decode_error(error: impl std::fmt::Display) -> ArtworkError {
    ArtworkError::Decode(error.to_string())
}
