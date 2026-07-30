use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use font8x8::{BASIC_FONTS, UnicodeFonts};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform,
};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::model::{LimitWindow, ResetCredits};

const WIDTH: u32 = 144;
const HEIGHT: u32 = 144;

#[derive(Clone, Debug)]
pub enum TileView {
    Unconfigured,
    Loading {
        label: String,
    },
    Limits {
        label: String,
        windows: Vec<LimitWindow>,
        reset_credits: Option<ResetCredits>,
        refreshing: bool,
        stale: bool,
    },
    Error {
        label: String,
        message: String,
    },
}

pub fn render_data_uri(view: &TileView) -> Result<String, String> {
    let png = render_png(view)?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

pub fn render_png(view: &TileView) -> Result<Vec<u8>, String> {
    let mut pixmap = Pixmap::new(WIDTH, HEIGHT).ok_or("failed to allocate tile image")?;
    pixmap.fill(color(12, 15, 19));
    let label = match view {
        TileView::Unconfigured => "CL",
        TileView::Loading { label }
        | TileView::Limits { label, .. }
        | TileView::Error { label, .. } => label,
    };
    draw_header(&mut pixmap, label);

    match view {
        TileView::Unconfigured => {
            draw_centered(&mut pixmap, "SET UP", 62, 2, color(226, 232, 237));
            draw_centered(&mut pixmap, "SELECT HOME", 91, 1, color(129, 142, 154));
        }
        TileView::Loading { .. } => {
            draw_centered(&mut pixmap, "LOADING", 65, 2, color(226, 232, 237));
            draw_centered(&mut pixmap, "...", 96, 2, color(65, 174, 255));
        }
        TileView::Error { message, .. } => {
            draw_centered(&mut pixmap, "!", 52, 4, color(232, 72, 72));
            draw_centered(
                &mut pixmap,
                &message.to_ascii_uppercase(),
                106,
                1,
                color(226, 232, 237),
            );
        }
        TileView::Limits {
            windows,
            reset_credits,
            refreshing,
            stale,
            ..
        } => {
            if windows.is_empty() {
                draw_centered(&mut pixmap, "NO LIMITS", 62, 2, color(129, 142, 154));
            } else if windows.len() == 1 {
                draw_window(&mut pixmap, &windows[0], 54);
            } else {
                draw_window(&mut pixmap, &windows[0], 34);
                draw_window(&mut pixmap, &windows[1], 73);
            }

            draw_reset_credits(&mut pixmap, reset_credits.as_ref(), unix_now());

            if *refreshing {
                fill_rect(&mut pixmap, 132.0, 10.0, 4.0, 4.0, color(65, 174, 255));
            } else if *stale {
                fill_rect(&mut pixmap, 132.0, 10.0, 4.0, 4.0, color(238, 164, 58));
            }
        }
    }

    pixmap.encode_png().map_err(|error| error.to_string())
}

fn draw_reset_credits(pixmap: &mut Pixmap, credits: Option<&ResetCredits>, now: i64) {
    let (count, count_color) = match credits {
        Some(credits) => (
            credits.available_count.to_string(),
            if credits.available_count > 0 {
                color(65, 174, 255)
            } else {
                color(129, 142, 154)
            },
        ),
        None => ("--".to_owned(), color(129, 142, 154)),
    };
    draw_reset_icon(pixmap, 14, 116, count_color);
    draw_compact_text(pixmap, &count, 33, 116, 2, 1, count_color);

    let Some(expires_at) = credits.and_then(|credits| credits.earliest_expires_at) else {
        return;
    };
    let expiry = reset_expiry_label(expires_at, now).to_ascii_uppercase();
    draw_compact_text(
        pixmap,
        &expiry,
        WIDTH as i32 - 14 - compact_text_width(&expiry, 2, 1),
        116,
        2,
        1,
        reset_expiry_color(expires_at.saturating_sub(now)),
    );
}

fn draw_reset_icon(pixmap: &mut Pixmap, x: i32, y: i32, icon_color: Color) {
    // Keep the arrowhead tangential to the arc. A radial or open-corner tip
    // becomes ambiguous at key resolution and can resemble a lowercase letter.
    let x = x as f32;
    let y = y as f32;
    let mut arc = PathBuilder::new();
    arc.move_to(x + 3.0, y + 10.0);
    arc.cubic_to(x + 4.0, y + 14.0, x + 7.0, y + 15.0, x + 10.0, y + 14.0);
    arc.cubic_to(x + 14.0, y + 13.0, x + 15.0, y + 9.0, x + 14.0, y + 6.0);
    arc.cubic_to(x + 13.0, y + 2.0, x + 8.0, y + 1.0, x + 5.0, y + 3.0);
    let Some(arc) = arc.finish() else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(icon_color);
    paint.anti_alias = false;
    let stroke = Stroke {
        width: 2.0,
        line_cap: LineCap::Square,
        line_join: LineJoin::Miter,
        ..Stroke::default()
    };
    pixmap.stroke_path(&arc, &paint, &stroke, Transform::identity(), None);

    let mut arrowhead = PathBuilder::new();
    arrowhead.move_to(x + 1.0, y + 7.0);
    arrowhead.line_to(x + 3.0, y + 1.0);
    arrowhead.line_to(x + 7.0, y + 5.0);
    arrowhead.close();
    if let Some(arrowhead) = arrowhead.finish() {
        pixmap.fill_path(
            &arrowhead,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResetExpiryUrgency {
    Healthy,
    Warning,
    Critical,
}

fn reset_expiry_urgency(remaining_seconds: i64) -> ResetExpiryUrgency {
    match remaining_seconds {
        ..=86_400 => ResetExpiryUrgency::Critical,
        86_401..=604_800 => ResetExpiryUrgency::Warning,
        _ => ResetExpiryUrgency::Healthy,
    }
}

fn reset_expiry_color(remaining_seconds: i64) -> Color {
    match reset_expiry_urgency(remaining_seconds) {
        ResetExpiryUrgency::Healthy => color(58, 190, 126),
        ResetExpiryUrgency::Warning => color(238, 164, 58),
        ResetExpiryUrgency::Critical => color(232, 72, 72),
    }
}

fn reset_expiry_label(expires_at: i64, now: i64) -> String {
    let remaining = expires_at.saturating_sub(now);
    if remaining <= 0 {
        return "NOW".to_owned();
    }
    if remaining < 3_600 {
        return "<1h".to_owned();
    }

    let total_hours = remaining / 3_600;
    let days = total_hours / 24;
    let hours = total_hours % 24;
    match (days, hours) {
        (0, hours) => format!("{hours}h"),
        (days, 0) => format!("{days}d"),
        (days, hours) => format!("{days}d {hours}h"),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn draw_header(pixmap: &mut Pixmap, label: &str) {
    const MAX_CHARACTERS: usize = 10;
    const SPACING: i32 = 3;

    let normalized = normalize_header_label(label);
    let label = truncate_with_ellipsis(&normalized, MAX_CHARACTERS);
    let width = compact_text_width(&label, 2, SPACING);
    draw_compact_text(
        pixmap,
        &label,
        (WIDTH as i32 - width) / 2,
        8,
        2,
        SPACING,
        color(226, 232, 237),
    );
}

fn compact_text_width(text: &str, scale: i32, spacing: i32) -> i32 {
    let characters = text.chars().count() as i32;
    if characters == 0 {
        0
    } else {
        characters * 5 * scale + (characters - 1) * spacing
    }
}

fn draw_compact_text(
    pixmap: &mut Pixmap,
    text: &str,
    x: i32,
    y: i32,
    scale: i32,
    spacing: i32,
    text_color: Color,
) {
    let mut cursor = x;
    for character in text.chars() {
        let glyph = header_glyph(character);
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    fill_rect(
                        pixmap,
                        (cursor + column * scale) as f32,
                        (y + row as i32 * scale) as f32,
                        scale as f32,
                        scale as f32,
                        text_color,
                    );
                }
            }
        }
        cursor += 5 * scale + spacing;
    }
}

fn normalize_header_label(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_uppercase)
        .collect::<String>()
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .map(|character| match character {
            'A'..='Z' | '0'..='9' | '-' | '_' | '.' | ' ' => character,
            character if character.is_whitespace() => ' ',
            'Æ' | 'Ǣ' | 'Ǽ' => 'A',
            'Ð' | 'Đ' => 'D',
            'Ł' => 'L',
            'Ø' => 'O',
            'Œ' => 'O',
            'Þ' => 'T',
            _ => '?',
        })
        .collect()
}

#[rustfmt::skip]
fn header_glyph(character: char) -> [u8; 7] {
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
        '<' => [0b00001, 0b00010, 0b00100, 0b01000, 0b00100, 0b00010, 0b00001],
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

fn draw_window(pixmap: &mut Pixmap, window: &LimitWindow, y: i32) {
    let accent = quota_color(window.remaining_percent);
    let mut label = window.label();
    let percentage = format!("{}%", window.remaining_percent);
    let label_color = color(154, 167, 178);
    let label_width = text_width(&label, 2);
    let compact_label_width = compact_text_width(&label, 2, 1);
    let mut percentage_width = text_width(&percentage, 2);
    let mut percentage_x = WIDTH as i32 - 14 - percentage_width;
    let use_compact_label = 14 + label_width + 6 > percentage_x;
    let use_compact_percentage = use_compact_label && 14 + compact_label_width + 6 > percentage_x;

    if use_compact_percentage {
        percentage_width = compact_text_width(&percentage, 2, 1);
        percentage_x = WIDTH as i32 - 14 - percentage_width;

        let label_space = (percentage_x - 6 - 14).max(0);
        let label_capacity = ((label_space + 1) / 11).max(1) as usize;
        label = truncate_with_ellipsis(&label, label_capacity);
    }

    // Keep the duration and percentage on one baseline. Long future duration
    // labels use the narrower tile alphabet at nearly the same cap height
    // instead of shrinking the entire row.
    if !use_compact_label {
        draw_text(pixmap, &label, 14, y, 2, label_color);
    } else {
        draw_compact_text(pixmap, &label, 14, y + 1, 2, 1, label_color);
    }
    if use_compact_percentage {
        draw_compact_text(
            pixmap,
            &percentage,
            percentage_x,
            y + 1,
            2,
            1,
            color(238, 242, 245),
        );
    } else {
        draw_text(
            pixmap,
            &percentage,
            percentage_x,
            y,
            2,
            color(238, 242, 245),
        );
    }

    let bar_y = y + 23;
    fill_rect(pixmap, 14.0, bar_y as f32, 116.0, 7.0, color(37, 44, 51));
    let width = 116.0 * f32::from(window.remaining_percent) / 100.0;
    if width > 0.0 {
        fill_rect(pixmap, 14.0, bar_y as f32, width, 7.0, accent);
    }
}

fn quota_color(remaining: u8) -> Color {
    match remaining {
        50..=100 => color(58, 190, 126),
        20..=49 => color(238, 164, 58),
        _ => color(232, 72, 72),
    }
}

fn truncate_with_ellipsis(value: &str, max_characters: usize) -> String {
    if value.chars().count() <= max_characters {
        return value.to_owned();
    }
    if max_characters <= 3 {
        return ".".repeat(max_characters);
    }

    let prefix = value.chars().take(max_characters - 3).collect::<String>();
    format!("{}...", prefix.trim_end())
}

fn draw_centered(pixmap: &mut Pixmap, text: &str, y: i32, scale: i32, text_color: Color) {
    let width = text_width(text, scale);
    draw_text(
        pixmap,
        text,
        ((WIDTH as i32 - width) / 2).max(6),
        y,
        scale,
        text_color,
    );
}

fn text_width(text: &str, scale: i32) -> i32 {
    let characters = text.chars().count() as i32;
    if characters == 0 {
        0
    } else {
        characters * 8 * scale + (characters - 1) * scale
    }
}

fn draw_text(pixmap: &mut Pixmap, text: &str, x: i32, y: i32, scale: i32, text_color: Color) {
    let mut cursor = x;
    for character in text.chars() {
        let glyph_y = if character == '~' { y + scale * 2 } else { y };
        let glyph = BASIC_FONTS
            .get(character)
            .or_else(|| BASIC_FONTS.get('?'))
            .unwrap_or([0; 8]);
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..8 {
                if bits & (1 << column) != 0 {
                    fill_rect(
                        pixmap,
                        (cursor + column * scale) as f32,
                        (glyph_y + row as i32 * scale) as f32,
                        scale as f32,
                        scale as f32,
                        text_color,
                    );
                }
            }
        }
        cursor += 9 * scale;
    }
}

fn fill_rect(pixmap: &mut Pixmap, x: f32, y: f32, width: f32, height: f32, fill: Color) {
    let Some(rect) = Rect::from_xywh(x, y, width, height) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(fill);
    paint.anti_alias = false;
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

fn color(red: u8, green: u8, blue: u8) -> Color {
    Color::from_rgba8(red, green, blue, 255)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(remaining: u8, duration: u64) -> LimitWindow {
        LimitWindow {
            used_percent: 100 - remaining,
            remaining_percent: remaining,
            duration_minutes: Some(duration),
            resets_at: None,
        }
    }

    fn resets(count: u64, expires_at: Option<i64>) -> Option<ResetCredits> {
        Some(ResetCredits {
            available_count: count,
            earliest_expires_at: expires_at,
        })
    }

    #[test]
    fn renders_all_tile_states_as_144_square_pngs() {
        let states = [
            TileView::Unconfigured,
            TileView::Loading {
                label: "PERSONAL".into(),
            },
            TileView::Limits {
                label: "CONTEXTIVO".into(),
                windows: vec![window(93, 300), window(58, 10_080)],
                reset_credits: resets(2, Some(unix_now() + 3 * 86_400 + 4 * 3_600)),
                refreshing: false,
                stale: false,
            },
            TileView::Limits {
                label: "PLUS".into(),
                windows: vec![window(17, 43_800)],
                reset_credits: resets(0, None),
                refreshing: true,
                stale: false,
            },
            TileView::Limits {
                label: "RĪGA ACCOUNT".into(),
                windows: vec![window(44, 10_080)],
                reset_credits: None,
                refreshing: false,
                stale: true,
            },
            TileView::Error {
                label: "OITG".into(),
                message: "Offline".into(),
            },
        ];

        let preview_directory =
            std::env::var_os("CODEX_LIMITS_RENDER_DIR").map(std::path::PathBuf::from);
        if let Some(directory) = &preview_directory {
            std::fs::create_dir_all(directory).unwrap();
        }

        for (index, state) in states.into_iter().enumerate() {
            let png = render_png(&state).unwrap();
            assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
            let image = tiny_skia::Pixmap::decode_png(&png).unwrap();
            assert_eq!(image.width(), WIDTH);
            assert_eq!(image.height(), HEIGHT);
            if let Some(directory) = &preview_directory {
                std::fs::write(directory.join(format!("tile-{index}.png")), png).unwrap();
            }
        }
    }

    #[test]
    fn truncates_long_headers_with_an_ellipsis() {
        assert_eq!(truncate_with_ellipsis("CONTEXTIVO", 10), "CONTEXTIVO");
        assert_eq!(truncate_with_ellipsis("CODEX CONTEXTIVO", 10), "CODEX C...");
        assert_eq!(compact_text_width("PLUS", 2, 3), 49);
        assert!(compact_text_width("CONTEXTIVO", 2, 3) < WIDTH as i32);
    }

    #[test]
    fn transliterates_unicode_headers_before_drawing() {
        assert_eq!(normalize_header_label("Pēteris"), "PETERIS");
        assert_eq!(normalize_header_label("Crème Brûlée"), "CREME BRULEE");

        let png = render_png(&TileView::Limits {
            label: "Rīga ĀŽ".into(),
            windows: vec![window(88, 10_080)],
            reset_credits: resets(12, Some(unix_now() + 12 * 86_400 + 23 * 3_600)),
            refreshing: false,
            stale: false,
        })
        .unwrap();
        assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn formats_the_first_reset_expiry_as_days_and_hours() {
        let now = 1_800_000_000;
        assert_eq!(
            reset_expiry_label(now + 3 * 86_400 + 4 * 3_600, now),
            "3d 4h"
        );
        assert_eq!(reset_expiry_label(now + 2 * 86_400, now), "2d");
        assert_eq!(reset_expiry_label(now + 7 * 3_600, now), "7h");
        assert_eq!(reset_expiry_label(now + 3_599, now), "<1h");
        assert_eq!(reset_expiry_label(now, now), "NOW");
    }

    #[test]
    fn changes_reset_expiry_urgency_as_time_runs_out() {
        assert_eq!(
            reset_expiry_urgency(8 * 86_400),
            ResetExpiryUrgency::Healthy
        );
        assert_eq!(
            reset_expiry_urgency(7 * 86_400),
            ResetExpiryUrgency::Warning
        );
        assert_eq!(reset_expiry_urgency(86_401), ResetExpiryUrgency::Warning);
        assert_eq!(reset_expiry_urgency(86_400), ResetExpiryUrgency::Critical);
        assert_eq!(reset_expiry_urgency(0), ResetExpiryUrgency::Critical);
    }
}
