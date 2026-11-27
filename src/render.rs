use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform,
};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

mod text;

use self::text::Font;
use crate::usage::{LimitWindow, ResetCredits, UsageSnapshot};

const WIDTH: u32 = 144;
const HEIGHT: u32 = 144;
const FOOTER_TEXT_Y: i32 = 116;
const VALUE_FONT: Font = Font::basic(2);
const COMPACT_VALUE_FONT: Font = Font::compact(2, 1);
const MARGIN: i32 = 14;
const FOOTER_ICON_Y: i32 = FOOTER_TEXT_Y - 1;

#[derive(Clone, Debug)]
pub enum TileView {
    Unconfigured,
    Loading {
        label: String,
    },
    Limits {
        label: String,
        snapshot: UsageSnapshot,
        freshness: Freshness,
    },
    Error {
        label: String,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Ready,
    Refreshing,
    Stale,
}

pub fn render_data_uri(view: &TileView) -> Result<String, String> {
    let png = render_png(view, unix_now())?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

fn render_png(view: &TileView, now: i64) -> Result<Vec<u8>, String> {
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
            snapshot,
            freshness,
            ..
        } => {
            match snapshot.windows.as_slice() {
                [] => draw_centered(&mut pixmap, "NO LIMITS", 62, 2, color(129, 142, 154)),
                [only] => draw_window(&mut pixmap, only, 45, now),
                [first, second, ..] => {
                    draw_window(&mut pixmap, first, 27, now);
                    draw_window(&mut pixmap, second, 70, now);
                }
            }
            draw_reset_credits(&mut pixmap, snapshot.reset_credits.as_ref(), now);

            let indicator = match freshness {
                Freshness::Ready => None,
                Freshness::Refreshing => Some(color(65, 174, 255)),
                Freshness::Stale => Some(color(238, 164, 58)),
            };
            if let Some(indicator) = indicator {
                fill_rect(&mut pixmap, 132.0, 10.0, 4.0, 4.0, indicator);
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
    draw_reset_icon(pixmap, MARGIN, FOOTER_ICON_Y, count_color);
    COMPACT_VALUE_FONT.draw(pixmap, &count, MARGIN + 19, FOOTER_TEXT_Y, count_color);

    let Some(expires_at) = credits.and_then(|credits| credits.earliest_expires_at) else {
        return;
    };
    let remaining = expires_at.saturating_sub(now);
    draw_right_value(
        pixmap,
        &countdown_label(remaining),
        FOOTER_TEXT_Y,
        reset_expiry_color(remaining),
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

fn reset_expiry_color(remaining_seconds: i64) -> Color {
    match remaining_seconds {
        ..=86_400 => color(232, 72, 72),
        86_401..=604_800 => color(238, 164, 58),
        _ => color(58, 190, 126),
    }
}

fn countdown_label(remaining: i64) -> String {
    if remaining <= 0 {
        return "NOW".to_owned();
    }
    if remaining < 3_600 {
        return format!("{}M", (remaining / 60).max(1));
    }

    let total_hours = remaining / 3_600;
    let days = total_hours / 24;
    let hours = total_hours % 24;
    let minutes = (remaining / 60) % 60;
    match (days, hours) {
        (0, hours) if minutes > 0 => format!("{hours}H {minutes}M"),
        (0, hours) => format!("{hours}H"),
        (days, 0) => format!("{days}D"),
        (days, hours) => format!("{days}D {hours}H"),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn draw_header(pixmap: &mut Pixmap, label: &str) {
    const HEADER_FONT: Font = Font::compact(2, 3);
    let normalized = normalize_header_label(label);
    let label = truncate_with_ellipsis(&normalized, 10);
    HEADER_FONT.draw(
        pixmap,
        &label,
        (WIDTH as i32 - HEADER_FONT.width(&label)) / 2,
        8,
        color(226, 232, 237),
    );
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

fn draw_window(pixmap: &mut Pixmap, window: &LimitWindow, y: i32, now: i64) {
    let accent = quota_color(window.remaining_percent);
    draw_window_values(
        pixmap,
        &window.label(),
        &format!("{}%", window.remaining_percent),
        y,
    );
    const BAR_X: f32 = MARGIN as f32;
    const BAR_WIDTH: f32 = WIDTH as f32 - 2.0 * BAR_X;
    let time_left = window.time_remaining_percent(now);
    let (bar_y, height) = if time_left.is_some() {
        ((y + 19) as f32, 4.0)
    } else {
        ((y + 18) as f32, 7.0)
    };
    fill_rect(pixmap, BAR_X, bar_y, BAR_WIDTH, height, color(37, 44, 51));
    if let Some(percent) = time_left {
        let center = (BAR_WIDTH * f32::from(percent) / 100.0).round();
        let x = BAR_X + (center - 1.0).clamp(0.0, BAR_WIDTH - 2.0);
        fill_rect(pixmap, x, bar_y - 3.0, 2.0, 10.0, color(238, 242, 245));
    }
    let width = BAR_WIDTH * f32::from(window.remaining_percent) / 100.0;
    if width > 0.0 {
        fill_rect(pixmap, BAR_X, bar_y, width, height, accent);
    }

    let reset_y = y + 27;
    let reset_time = window
        .resets_at
        .map(|at| countdown_label(at.saturating_sub(now)))
        .unwrap_or_else(|| "--".to_owned());
    COMPACT_VALUE_FONT.draw(pixmap, "IN", MARGIN, reset_y, color(129, 142, 154));
    draw_right_value(pixmap, &reset_time, reset_y, color(193, 203, 211));
}

fn draw_right_value(pixmap: &mut Pixmap, value: &str, y: i32, color: Color) {
    let x = WIDTH as i32 - MARGIN - COMPACT_VALUE_FONT.width(value);
    COMPACT_VALUE_FONT.draw(pixmap, value, x, y, color);
}

fn draw_window_values(pixmap: &mut Pixmap, label: &str, percentage: &str, y: i32) {
    let available = WIDTH as i32 - 2 * MARGIN - 6;
    let fits = |font: Font| font.width(label) + VALUE_FONT.width(percentage) <= available;
    let standard = (VALUE_FONT, y);
    let compact = (COMPACT_VALUE_FONT, y + 1);
    let ((label_font, label_y), (percentage_font, percentage_y)) = if fits(VALUE_FONT) {
        (standard, standard)
    } else if fits(COMPACT_VALUE_FONT) {
        (compact, standard)
    } else {
        (compact, compact)
    };
    let percentage_width = percentage_font.width(percentage);
    let label = truncate_with_ellipsis(
        label,
        label_font.capacity(available - percentage_width).max(1),
    );
    label_font.draw(pixmap, &label, MARGIN, label_y, color(154, 167, 178));
    percentage_font.draw(
        pixmap,
        percentage,
        WIDTH as i32 - MARGIN - percentage_width,
        percentage_y,
        color(238, 242, 245),
    );
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
    let font = Font::basic(scale);
    font.draw(
        pixmap,
        text,
        ((WIDTH as i32 - font.width(text)) / 2).max(6),
        y,
        text_color,
    );
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
            remaining_percent: remaining,
            duration_minutes: Some(duration),
            resets_at: None,
        }
    }

    #[test]
    fn draws_the_time_marker_independently_of_quota_and_within_the_track() {
        let now = 1_000;
        let tick = color(238, 242, 245).premultiply().to_color_u8();
        for (duration, seconds_left, expected_x) in [
            (Some(300), Some(4_600), Some(43)),
            (Some(10_080), Some(302_400), Some(71)),
            (Some(300), Some(-1), Some(14)),
            (Some(300), Some(20_000), Some(128)),
            (None, Some(300), None),
            (Some(0), Some(300), None),
            (Some(300), None, None),
        ] {
            let mut pixmap = Pixmap::new(WIDTH, HEIGHT).unwrap();
            let window = LimitWindow {
                duration_minutes: duration,
                resets_at: seconds_left.map(|seconds| now + seconds),
                ..window(60, 300)
            };
            draw_window(&mut pixmap, &window, 0, now);
            for y in [16, 17, 18, 23, 24, 25] {
                let marked = (0..WIDTH)
                    .filter(|&x| pixmap.pixel(x, y) == Some(tick))
                    .collect::<Vec<_>>();
                let expected = expected_x.map_or_else(Vec::new, |x| vec![x, x + 1]);
                assert_eq!(marked, expected);
            }
        }
    }

    #[test]
    fn keeps_the_last_quota_pixel_visible_and_preserves_unmarked_bar_height() {
        let mut pixmap = Pixmap::new(WIDTH, HEIGHT).unwrap();
        let mut window = window(1, 300);
        let red = color(232, 72, 72).premultiply().to_color_u8();
        draw_window(&mut pixmap, &window, 0, 0);
        for y in 18..25 {
            assert_eq!(pixmap.pixel(14, y), Some(red));
        }

        window.resets_at = Some(1);
        pixmap.fill(Color::TRANSPARENT);
        draw_window(&mut pixmap, &window, 0, 0);
        assert_eq!(pixmap.pixel(14, 20), Some(red));
        assert_eq!(
            pixmap.pixel(14, 17),
            Some(color(238, 242, 245).premultiply().to_color_u8())
        );
    }

    fn resets(count: u64, expires_at: Option<i64>) -> Option<ResetCredits> {
        Some(ResetCredits {
            available_count: count,
            earliest_expires_at: expires_at,
        })
    }

    fn tile_states(now: i64) -> [TileView; 6] {
        let window_reset = |remaining, duration, resets_in| LimitWindow {
            resets_at: Some(now + resets_in),
            ..window(remaining, duration)
        };
        [
            TileView::Unconfigured,
            TileView::Loading {
                label: "PERSONAL".into(),
            },
            TileView::Limits {
                label: "CONTEXTIVO".into(),
                snapshot: UsageSnapshot {
                    windows: vec![
                        window_reset(93, 300, 4 * 3_600),
                        window_reset(58, 10_080, 2 * 86_400 + 7 * 3_600),
                    ],
                    reset_credits: resets(2, Some(now + 3 * 86_400 + 4 * 3_600)),
                },
                freshness: Freshness::Ready,
            },
            TileView::Limits {
                label: "PLUS".into(),
                snapshot: UsageSnapshot {
                    windows: vec![window_reset(17, 43_800, 18 * 86_400 + 9 * 3_600)],
                    reset_credits: resets(0, None),
                },
                freshness: Freshness::Refreshing,
            },
            TileView::Limits {
                label: "RĪGA ACCOUNT".into(),
                snapshot: UsageSnapshot {
                    windows: vec![window(44, 10_080)],
                    reset_credits: None,
                },
                freshness: Freshness::Stale,
            },
            TileView::Error {
                label: "OITG".into(),
                message: "Offline".into(),
            },
        ]
    }

    #[test]
    #[ignore = "manual release-mode performance measurement"]
    fn benchmark_rendering() {
        let states = tile_states(unix_now());
        let mut samples = Vec::new();
        for _ in 0..7 {
            let start = std::time::Instant::now();
            for _ in 0..200 {
                for state in &states {
                    std::hint::black_box(render_data_uri(std::hint::black_box(state)).unwrap());
                }
            }
            samples.push(start.elapsed().as_nanos() / (200 * states.len()) as u128);
        }
        samples.sort_unstable();
        println!(
            "render + PNG + base64: {} ns/tile (median of 7 samples)",
            samples[3]
        );
    }

    #[test]
    fn renders_all_tile_states_as_144_square_pngs() {
        let now = 1_800_000_000;
        let states = tile_states(now);

        let preview_directory =
            std::env::var_os("CODEX_LIMITS_RENDER_DIR").map(std::path::PathBuf::from);
        if let Some(directory) = &preview_directory {
            std::fs::create_dir_all(directory).unwrap();
        }

        for (index, state) in states.into_iter().enumerate() {
            let png = render_png(&state, now).unwrap();
            let image = tiny_skia::Pixmap::decode_png(&png).unwrap();
            assert_eq!((image.width(), image.height()), (144, 144));
            if let Some(directory) = &preview_directory {
                std::fs::write(directory.join(format!("tile-{index}.png")), png).unwrap();
            }
        }
    }

    fn header_pixels(label: &str) -> Pixmap {
        let mut pixmap = Pixmap::new(WIDTH, HEIGHT).unwrap();
        draw_header(&mut pixmap, label);
        pixmap
    }

    #[test]
    fn truncates_and_transliterates_rendered_headers() {
        for (label, expected) in [
            ("CODEX CONTEXTIVO", "CODEX C..."),
            ("Pēteris", "PETERIS"),
            ("Crème Brûl", "CREME BRUL"),
        ] {
            let header = header_pixels(label);
            assert_ne!(header, header_pixels(""));
            assert_eq!(header, header_pixels(expected));
        }
    }

    fn occupied_bounds(
        pixmap: &Pixmap,
        ink: Color,
        xs: std::ops::Range<u32>,
        ys: std::ops::Range<u32>,
    ) -> (u32, u32, u32, u32) {
        let pixel = ink.premultiply().to_color_u8();
        let mut bounds = None;
        for y in ys {
            for x in xs.clone() {
                if pixmap.pixel(x, y) == Some(pixel) {
                    let (left, top, right, bottom) = bounds.unwrap_or((x, y, x, y));
                    bounds = Some((left.min(x), top.min(y), right.max(x), bottom.max(y)));
                }
            }
        }
        bounds.expect("expected visible ink")
    }

    #[test]
    fn keeps_reset_countdowns_readable_beside_the_cue() {
        for resets_at in [
            999 * 86_400 + 23 * 3600,
            23 * 3600 + 59 * 60,
            4 * 3600 + 59 * 60,
        ] {
            let mut pixmap = Pixmap::new(WIDTH, HEIGHT).unwrap();
            let window = LimitWindow {
                resets_at: Some(resets_at),
                ..window(50, 300)
            };
            draw_window(&mut pixmap, &window, 0, 0);
            let (_, _, cue_right, _) =
                occupied_bounds(&pixmap, color(129, 142, 154), 0..WIDTH, 27..45);
            let (left, top, _, bottom) =
                occupied_bounds(&pixmap, color(193, 203, 211), 0..WIDTH, 27..45);
            assert!(cue_right + 1 < left);
            assert_eq!(bottom - top + 1, 14);
        }
    }

    #[test]
    fn formats_reset_countdowns_as_days_hours_and_minutes() {
        assert_eq!(countdown_label(3 * 86_400 + 4 * 3_600), "3D 4H");
        assert_eq!(countdown_label(2 * 86_400), "2D");
        assert_eq!(countdown_label(7 * 3_600), "7H");
        assert_eq!(countdown_label(5 * 3_600), "5H");
        assert_eq!(countdown_label(5 * 3_600 - 1), "4H 59M");
        assert_eq!(countdown_label(3_660), "1H 1M");
        assert_eq!(countdown_label(3_600), "1H");
        assert_eq!(countdown_label(86_399), "23H 59M");
        assert_eq!(countdown_label(86_400), "1D");
        assert_eq!(countdown_label(3_599), "59M");
        assert_eq!(countdown_label(60), "1M");
        assert_eq!(countdown_label(1), "1M");
        assert_eq!(countdown_label(0), "NOW");
    }

    #[test]
    fn changes_reset_expiry_urgency_as_time_runs_out() {
        assert_eq!(reset_expiry_color(8 * 86_400), color(58, 190, 126));
        assert_eq!(reset_expiry_color(7 * 86_400), color(238, 164, 58));
        assert_eq!(reset_expiry_color(86_401), color(238, 164, 58));
        assert_eq!(reset_expiry_color(86_400), color(232, 72, 72));
        assert_eq!(reset_expiry_color(0), color(232, 72, 72));
    }

    #[test]
    fn aligns_the_reset_icon_with_the_footer_text() {
        let mut pixmap = Pixmap::new(WIDTH, HEIGHT).unwrap();
        let credits = ResetCredits {
            available_count: 2,
            earliest_expires_at: None,
        };
        draw_reset_credits(&mut pixmap, Some(&credits), 0);
        let ink = color(65, 174, 255);
        let (_, icon_top, _, icon_bottom) = occupied_bounds(&pixmap, ink, 0..32, 100..HEIGHT);
        let (_, count_top, _, count_bottom) = occupied_bounds(&pixmap, ink, 32..WIDTH, 100..HEIGHT);
        assert_eq!((icon_top, icon_bottom), (count_top, count_bottom));
    }
}
