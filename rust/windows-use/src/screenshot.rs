//! Screen capture and screenshot annotation, mirroring the Python `Pillow` usage.

use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
    DIB_RGB_COLORS, HBITMAP, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// An RGBA screenshot of the virtual screen.
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Screenshot {
    /// Encodes the screenshot as PNG.
    pub fn to_png(&self) -> anyhow::Result<Vec<u8>> {
        let image = self.to_image();
        let mut buffer = std::io::Cursor::new(Vec::new());
        image.write_to(&mut buffer, image::ImageFormat::Png)?;
        Ok(buffer.into_inner())
    }

    pub fn to_image(&self) -> image::RgbaImage {
        image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
            .expect("screenshot buffer size")
    }
}

/// Captures the whole virtual screen (all monitors) as BGRA→RGBA.
pub fn capture_virtual_screen() -> anyhow::Result<Screenshot> {
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);

        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            anyhow::bail!("GetDC failed");
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        let bitmap: HBITMAP = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.is_invalid() {
            ReleaseDC(None, screen_dc);
            anyhow::bail!("CreateCompatibleBitmap failed");
        }
        let previous = SelectObject(memory_dc, HGDIOBJ(bitmap.0));

        let copied = BitBlt(
            memory_dc,
            0,
            0,
            width,
            height,
            screen_dc,
            left,
            top,
            SRCCOPY | CAPTUREBLT,
        );
        if copied.is_err() {
            SelectObject(memory_dc, previous);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory_dc);
            ReleaseDC(None, screen_dc);
            anyhow::bail!("BitBlt failed");
        }

        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;

        let mut buffer = vec![0u8; (width * height * 4) as usize];
        let scanlines = GetDIBits(
            memory_dc,
            bitmap,
            0,
            height as u32,
            Some(buffer.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        );

        SelectObject(memory_dc, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory_dc);
        ReleaseDC(None, screen_dc);

        if scanlines == 0 {
            anyhow::bail!("GetDIBits failed");
        }

        // GDI gives BGRA; convert to RGBA and force opaque alpha.
        for pixel in buffer.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }

        Ok(Screenshot {
            width: width as u32,
            height: height as u32,
            rgba: buffer,
        })
    }
}

/// Virtual screen origin and size, matching `GetVirtualScreenRect`.
pub fn virtual_screen_rect() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    }
}

/// Draws element labels on a copy of the screenshot, matching `get_annotated_screenshot`.
pub fn annotate_screenshot(
    screenshot: &Screenshot,
    boxes: &[(i32, i32, i32, i32)],
    padding: u32,
) -> Screenshot {
    let pad = padding.max(1);
    let width = screenshot.width + pad * 3 / 2;
    let height = screenshot.height + pad * 3 / 2;
    let mut canvas = image::RgbaImage::from_pixel(width, height, image::Rgba([255, 255, 255, 255]));

    let source = screenshot.to_image();
    image::imageops::overlay(&mut canvas, &source, pad as i64, pad as i64);
    let (left_offset, top_offset, _, _) = virtual_screen_rect();

    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for (label, (left, top, right, bottom)) in boxes.iter().enumerate() {
        seed ^= (label as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let red = ((seed >> 16) & 0xFF) as u8;
        let green = ((seed >> 24) & 0xFF) as u8;
        let blue = ((seed >> 32) & 0xFF) as u8;
        let color = image::Rgba([red, green, blue, 255]);

        let x1 = (left - left_offset) + pad as i32;
        let y1 = (top - top_offset) + pad as i32;
        let x2 = (right - left_offset) + pad as i32;
        let y2 = (bottom - top_offset) + pad as i32;

        draw_rect_outline(&mut canvas, x1, y1, x2, y2, color, 2);

        // Label above the box, right-aligned like the Python renderer.
        let digits = label.to_string();
        let character_width = 7i32;
        let label_width = digits.len() as i32 * character_width + 4;
        let label_height = 14i32;
        let label_x1 = x2 - label_width;
        let label_y1 = y1 - label_height - 4;
        draw_filled_rect(
            &mut canvas,
            label_x1,
            label_y1,
            label_x1 + label_width,
            label_y1 + label_height,
            color,
        );
        draw_text(
            &mut canvas,
            &digits,
            label_x1 + 2,
            label_y1 + 2,
            image::Rgba([255, 255, 255, 255]),
        );
    }

    Screenshot { width, height, rgba: canvas.into_raw() }
}

fn draw_rect_outline(
    canvas: &mut image::RgbaImage,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    color: image::Rgba<u8>,
    thickness: i32,
) {
    for offset in 0..thickness {
        for x in x1..=x2 {
            put_pixel(canvas, x, y1 + offset, color);
            put_pixel(canvas, x, y2 - offset, color);
        }
        for y in y1..=y2 {
            put_pixel(canvas, x1 + offset, y, color);
            put_pixel(canvas, x2 - offset, y, color);
        }
    }
}

fn draw_filled_rect(
    canvas: &mut image::RgbaImage,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    color: image::Rgba<u8>,
) {
    for y in y1..=y2 {
        for x in x1..=x2 {
            put_pixel(canvas, x, y, color);
        }
    }
}

fn put_pixel(canvas: &mut image::RgbaImage, x: i32, y: i32, color: image::Rgba<u8>) {
    if x < 0 || y < 0 || x >= canvas.width() as i32 || y >= canvas.height() as i32 {
        return;
    }
    canvas.put_pixel(x as u32, y as u32, color);
}

/// Minimal 5x7 bitmap font, enough to render label digits.
fn draw_text(canvas: &mut image::RgbaImage, text: &str, x: i32, y: i32, color: image::Rgba<u8>) {
    let mut cursor = x;
    for character in text.chars() {
        let glyph = glyph_for(character);
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    put_pixel(canvas, cursor + column as i32, y + row as i32, color);
                }
            }
        }
        cursor += 6;
    }
}

fn glyph_for(character: char) -> [u8; 7] {
    match character {
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        _ => [0, 0, 0, 0, 0, 0, 0],
    }
}
