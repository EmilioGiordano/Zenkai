use tray_icon::Icon;

use super::TrayError;
use super::presence::Presence;

const APP_ICON: &[u8] = include_bytes!("../../zenkai.ico");
const MARK_SIZE: u32 = 32;
const HEADER_BYTES: usize = 6;
const ENTRY_BYTES: usize = 16;
const RING_RGB: [u8; 3] = [0x0d, 0x11, 0x17];

pub fn build(presence: Presence) -> Result<Icon, TrayError> {
    let (mut pixels, width, height) = decode_png(png_entry(APP_ICON, MARK_SIZE)?)?;
    draw_dot(&mut pixels, width, height, presence.dot_rgb());
    Icon::from_rgba(pixels, width, height).map_err(|error| TrayError::Icon(error.to_string()))
}

// Each entry of an .ico is (width, height, ..., size, offset) after a 6 byte header.
fn png_entry(ico: &[u8], size: u32) -> Result<&[u8], TrayError> {
    let missing = || TrayError::Icon(format!("no {size} pixel image in the application icon"));
    let count = read_u16(ico, 4).ok_or_else(missing)?;
    for index in 0..count {
        let start = HEADER_BYTES + index * ENTRY_BYTES;
        let entry = ico.get(start..start + ENTRY_BYTES).ok_or_else(missing)?;
        if u32::from(entry[0]) != size {
            continue;
        }
        let length = read_u32(entry, 8).ok_or_else(missing)?;
        let offset = read_u32(entry, 12).ok_or_else(missing)?;
        return ico.get(offset..offset + length).ok_or_else(missing);
    }
    Err(missing())
}

fn read_u16(bytes: &[u8], at: usize) -> Option<usize> {
    let raw: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(usize::from(u16::from_le_bytes(raw)))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<usize> {
    let raw: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    usize::try_from(u32::from_le_bytes(raw)).ok()
}

fn decode_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), TrayError> {
    let failed = |error: png::DecodingError| TrayError::Icon(error.to_string());
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .map_err(failed)?;
    let mut pixels = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut pixels).map_err(failed)?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(TrayError::Icon("the application icon is not RGBA8".into()));
    }
    pixels.truncate(info.buffer_size());
    Ok((pixels, info.width, info.height))
}

// A dot in the bottom right corner with a dark ring so it reads on any taskbar colour.
fn draw_dot(pixels: &mut [u8], width: u32, height: u32, rgb: [u8; 3]) {
    let radius = width as f32 * 0.24;
    let ring = (width as f32 / 32.0).max(1.0);
    let center_x = width as f32 - radius - 0.5;
    let center_y = height as f32 - radius - 0.5;
    for y in 0..height {
        for x in 0..width {
            let distance = ((x as f32 - center_x).powi(2) + (y as f32 - center_y).powi(2)).sqrt();
            let color = if distance <= radius - ring {
                rgb
            } else if distance <= radius {
                RING_RGB
            } else {
                continue;
            };
            let at = ((y * width + x) * 4) as usize;
            if let Some(pixel) = pixels.get_mut(at..at + 4) {
                pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * width + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    #[test]
    fn the_application_icon_has_a_decodable_mark() {
        let (pixels, width, height) = decode_png(png_entry(APP_ICON, MARK_SIZE).unwrap()).unwrap();
        assert_eq!((width, height), (MARK_SIZE, MARK_SIZE));
        assert_eq!(pixels.len(), (width * height * 4) as usize);
    }

    #[test]
    fn a_size_the_icon_does_not_have_is_an_error() {
        assert!(png_entry(APP_ICON, 31).is_err());
        assert!(png_entry(&[0, 0, 1, 0], 32).is_err());
    }

    #[test]
    fn the_dot_sits_in_the_corner_and_leaves_the_rest_alone() {
        let mut pixels = vec![10; 32 * 32 * 4];
        draw_dot(&mut pixels, 32, 32, Presence::Shown.dot_rgb());
        assert_eq!(pixel(&pixels, 32, 0, 0), [10; 4]);
        assert_eq!(pixel(&pixels, 32, 16, 16), [10; 4]);
        let [red, green, blue] = Presence::Shown.dot_rgb();
        assert_eq!(pixel(&pixels, 32, 24, 24), [red, green, blue, 255]);
    }

    #[test]
    fn the_two_states_draw_different_dots() {
        let mut shown = vec![0; 32 * 32 * 4];
        let mut hidden = vec![0; 32 * 32 * 4];
        draw_dot(&mut shown, 32, 32, Presence::Shown.dot_rgb());
        draw_dot(&mut hidden, 32, 32, Presence::HiddenToTray.dot_rgb());
        assert_ne!(pixel(&shown, 32, 24, 24), pixel(&hidden, 32, 24, 24));
    }
}
