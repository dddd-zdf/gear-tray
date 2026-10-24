use anyhow::Context;
use winit::window::Theme;

use crate::devices::{DeviceKind, PowerState};

/// A device silhouette above a percentage, drawn on a pixel-aligned 16px grid.
/// The doubled image stays crisp at the standard Windows tray size and on HiDPI.
pub fn generate_device_icon(
    kind: DeviceKind,
    theme: Theme,
    percent: Option<u8>,
    power: PowerState,
) -> anyhow::Result<tray_icon::Icon> {
    tray_icon::Icon::from_rgba(device_icon_rgba(kind, theme, percent, power, 32), 32, 32)
        .context("creating device battery tray icon")
}

/// Return the same tray artwork at a requested size for previews and image QA.
pub fn device_icon_rgba(
    kind: DeviceKind,
    theme: Theme,
    percent: Option<u8>,
    power: PowerState,
    size: u32,
) -> Vec<u8> {
    let unavailable = percent.is_none() || power == PowerState::Offline;
    let color = if unavailable {
        [145, 150, 160, 255]
    } else if kind == DeviceKind::Mouse && power == PowerState::Charging {
        [54, 211, 130, 255]
    } else if percent.is_some_and(|value| value <= 20) {
        [245, 174, 57, 255]
    } else if theme == Theme::Dark {
        [245, 247, 250, 255]
    } else {
        [35, 40, 47, 255]
    };
    let mut native = vec![0; 16 * 16 * 4];
    let (glyph, width, left): (&[u16], u32, u32) = match kind {
        DeviceKind::Headset => (
            &[
                0b0001111000,
                0b0010000100,
                0b0100000010,
                0b1100000011,
                0b1100000011,
                0b1100000011,
                0b0100000010,
            ],
            10,
            3,
        ),
        DeviceKind::Mouse => (
            &[
                0b00111100, 0b01011010, 0b10011001, 0b10000001, 0b10000001, 0b01000010, 0b00111100,
            ],
            8,
            4,
        ),
    };
    for (y, row) in glyph.iter().enumerate() {
        for x in 0..width {
            if row & (1 << (width - x - 1)) != 0 {
                put_pixel(&mut native, 16, left + x, y as u32, color);
            }
        }
    }
    if unavailable {
        for x in [3, 4, 5, 6, 9, 10, 11, 12] {
            put_pixel(&mut native, 16, x, 11, color);
            put_pixel(&mut native, 16, x, 12, color);
        }
    } else {
        let value = percent.unwrap_or_default().min(100);
        if value == 100 {
            // Narrow digit forms preserve the exact full-charge reading.
            let digits = [
                [0b010, 0b110, 0b010, 0b010, 0b010, 0b010, 0b111],
                [0b111, 0b101, 0b101, 0b101, 0b101, 0b101, 0b111],
            ];
            for (index, digit) in [0, 1, 1].into_iter().enumerate() {
                draw_compact_digit(&mut native, &digits[digit], 3, 2 + index as u32 * 4, color);
            }
        } else if value < 10 {
            draw_compact_digit(&mut native, &SMALL_DIGITS[value as usize], 5, 5, color);
        } else {
            draw_compact_digit(
                &mut native,
                &SMALL_DIGITS[(value / 10) as usize],
                5,
                2,
                color,
            );
            draw_compact_digit(
                &mut native,
                &SMALL_DIGITS[(value % 10) as usize],
                5,
                8,
                color,
            );
        }
    }
    let mut output = vec![0; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let source = (((y * 16 / size) * 16 + x * 16 / size) * 4) as usize;
            let target = ((y * size + x) * 4) as usize;
            output[target..target + 4].copy_from_slice(&native[source..source + 4]);
        }
    }
    output
}

fn draw_compact_digit(img: &mut [u8], rows: &[u8; 7], width: u32, left: u32, color: [u8; 4]) {
    for (y, row) in rows.iter().enumerate() {
        for x in 0..width {
            if row & (1 << (width - x - 1)) != 0 {
                put_pixel(img, 16, left + x, 9 + y as u32, color);
            }
        }
    }
}

const SMALL_DIGITS: [[u8; 7]; 10] = [
    [14, 17, 19, 21, 25, 17, 14],
    [4, 12, 4, 4, 4, 4, 14],
    [14, 17, 1, 2, 4, 8, 31],
    [30, 1, 1, 14, 1, 1, 30],
    [2, 6, 10, 18, 31, 2, 2],
    [31, 16, 16, 30, 1, 1, 30],
    [14, 16, 16, 30, 17, 17, 14],
    [31, 1, 2, 4, 8, 8, 8],
    [14, 17, 17, 14, 17, 17, 14],
    [14, 17, 17, 15, 1, 1, 14],
];

fn put_pixel(img: &mut [u8], size: u32, x: u32, y: u32, color: [u8; 4]) {
    let idx = ((y * size + x) * 4) as usize;
    if idx + 3 < img.len() {
        img[idx] = color[0];
        img[idx + 1] = color[1];
        img[idx + 2] = color[2];
        img[idx + 3] = color[3];
    }
}

