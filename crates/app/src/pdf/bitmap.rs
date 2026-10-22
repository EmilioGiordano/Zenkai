use std::sync::Arc;

use gpui_kit::RenderImage;
use image::{Frame, RgbaImage};

use super::protocol::Bitmap;

pub struct PageImage {
    pub image: Arc<RenderImage>,
    pub bytes: usize,
}

// GPUI wants BGRA, which is what pdfium renders, so the bytes go in as they are; the buffer
// type says RGBA only because that is the layout `image` has for four 8-bit channels.
pub fn to_image(bitmap: Bitmap) -> Option<PageImage> {
    let bytes = bitmap.bgra.len();
    let buffer = RgbaImage::from_raw(bitmap.width, bitmap.height, bitmap.bgra)?;
    Some(PageImage {
        image: Arc::new(RenderImage::new(vec![Frame::new(buffer)])),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bitmap_of_the_right_length_becomes_an_image_of_that_size() {
        let image = to_image(Bitmap {
            width: 2,
            height: 1,
            bgra: vec![0; 8],
        })
        .unwrap();
        assert_eq!(image.bytes, 8);
        assert_eq!(image.image.size(0).width.0, 2);
    }

    #[test]
    fn a_bitmap_of_the_wrong_length_is_refused() {
        let image = to_image(Bitmap {
            width: 2,
            height: 2,
            bgra: vec![0; 8],
        });
        assert!(image.is_none());
    }
}
