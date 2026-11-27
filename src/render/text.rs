use font8x8::{BASIC_FONTS, UnicodeFonts};
use tiny_skia::{Color, Pixmap};

#[derive(Clone, Copy)]
enum Face {
    Basic,
    Compact,
}

#[derive(Clone, Copy)]
pub(super) struct Font {
    face: Face,
    scale: i32,
    spacing: i32,
}

impl Font {
    pub(super) const fn basic(scale: i32) -> Self {
        Self {
            face: Face::Basic,
            scale,
            spacing: scale,
        }
    }

    pub(super) const fn compact(scale: i32, spacing: i32) -> Self {
        Self {
            face: Face::Compact,
            scale,
            spacing,
        }
    }

    fn advance(self) -> i32 {
        let columns = match self.face {
            Face::Basic => 8,
            Face::Compact => 5,
        };
        columns * self.scale + self.spacing
    }

    pub(super) fn width(self, text: &str) -> i32 {
        (text.chars().count() as i32 * self.advance() - self.spacing).max(0)
    }

    pub(super) fn capacity(self, width: i32) -> usize {
        ((width + self.spacing) / self.advance()).max(0) as usize
    }

    pub(super) fn draw(self, pixmap: &mut Pixmap, text: &str, mut x: i32, y: i32, color: Color) {
        for character in text.chars() {
            match self.face {
                Face::Basic => {
                    let glyph = BASIC_FONTS
                        .get(character)
                        .or_else(|| BASIC_FONTS.get('?'))
                        .unwrap_or([0; 8]);
                    let top = if character == '~' {
                        y + self.scale * 2
                    } else {
                        y
                    };
                    draw_glyph(pixmap, &glyph, x, top, self.scale, color);
                }
                Face::Compact => {
                    let glyph = compact_glyph(character).map(|row| row.reverse_bits() >> 3);
                    draw_glyph(pixmap, &glyph, x, y, self.scale, color);
                }
            }
            x += self.advance();
        }
    }
}

#[rustfmt::skip]
fn compact_glyph(character: char) -> [u8; 7] {
    match character {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b01010, 0b10001],
        'Y' => [0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
        '6' => [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        '~' => [0b00000, 0b00000, 0b01001, 0b10110, 0b00000, 0b00000, 0b00000],
        '%' => [0b11001, 0b11010, 0b00100, 0b01000, 0b10110, 0b00110, 0b00000],
        'd' => [0b00001, 0b00001, 0b01101, 0b10011, 0b10001, 0b10011, 0b01101],
        'h' => [0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001],
        'i' => [0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110],
        'm' => [0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10101, 0b10101],
        't' => [0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110],
        ' ' => [0; 7],
        _ => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100],
    }
}

// font8x8 encodes rows least-significant bit first.
fn draw_glyph(pixmap: &mut Pixmap, glyph: &[u8], x: i32, y: i32, scale: i32, fill: Color) {
    debug_assert_eq!(fill.alpha(), 1.0);
    debug_assert!(scale > 0);
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let pixel = fill.premultiply().to_color_u8();
    let pixels = pixmap.pixels_mut();

    for (row, bits) in glyph.iter().enumerate() {
        let top = y + row as i32 * scale;
        let bottom = (top + scale).clamp(0, height);
        for column in 0..8 {
            if bits & (1 << column) == 0 {
                continue;
            }
            let left = x + column * scale;
            let right = (left + scale).clamp(0, width) as usize;
            let left = left.clamp(0, width) as usize;
            for row in top.max(0)..bottom {
                let offset = row as usize * width as usize;
                pixels[offset + left..offset + right].fill(pixel);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{color, fill_rect};
    use super::*;

    #[test]
    fn bitmap_rasterization_matches_tiny_skia_including_clipped_cells() {
        let glyph = [0, 1, 0x80, 0xff, 0x55, 0xaa, 0x18, 0];
        let foreground = color(65, 174, 255);
        for scale in [1, 2, 4] {
            for x in [-32, -3, 0, 10, 23, 32] {
                for y in [-32, -3, 0, 10, 23, 32] {
                    let mut actual = Pixmap::new(24, 24).unwrap();
                    actual.fill(color(12, 15, 19));
                    let mut expected = actual.clone();
                    draw_glyph(&mut actual, &glyph, x, y, scale, foreground);
                    for (row, bits) in glyph.iter().enumerate() {
                        for column in 0..8 {
                            if bits & (1 << column) != 0 {
                                // Clip before invoking the reference rasterizer:
                                // its rounding can paint an extra edge pixel for
                                // rectangles ending at coordinate zero.
                                let left = (x + column * scale).max(0);
                                let top = (y + row as i32 * scale).max(0);
                                let right = (x + (column + 1) * scale).min(24);
                                let bottom = (y + (row as i32 + 1) * scale).min(24);
                                if right <= left || bottom <= top {
                                    continue;
                                }
                                fill_rect(
                                    &mut expected,
                                    left as f32,
                                    top as f32,
                                    (right - left) as f32,
                                    (bottom - top) as f32,
                                    foreground,
                                );
                            }
                        }
                    }
                    let mismatch = actual
                        .pixels()
                        .iter()
                        .zip(expected.pixels())
                        .position(|(left, right)| left != right);
                    assert!(
                        mismatch.is_none(),
                        "x={x}, y={y}, scale={scale}, mismatch={:?}",
                        mismatch.map(|index| (
                            index,
                            actual.pixels()[index],
                            expected.pixels()[index]
                        ))
                    );
                }
            }
        }
    }
}
