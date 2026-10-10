use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Keep the same terminal image id/encoding across frames and list selections.
pub(super) struct InlineImageProtocol {
    pub cells: (u16, u16),
    pub font_size: (u16, u16),
    pub protocol_type: ratatui_image::picker::ProtocolType,
    pub protocol: ratatui_image::protocol::StatefulProtocol,
}

pub(super) fn render_image_protocol(
    frame: &mut ratatui::Frame,
    area: Rect,
    protocol: &mut ratatui_image::protocol::StatefulProtocol,
) {
    frame.render_stateful_widget(ratatui_image::StatefulImage::default(), area, protocol);
    // Native protocols put escape payloads in an anchor cell and mark the
    // remaining image cells as skipped. Ratatui otherwise counts the escape
    // bytes as visible columns, skipping nearby text or another image's
    // first transmission. Keep the library's skip markers and give each
    // payload anchor one cell in the buffer diff.
    for y in area.top()..area.bottom() {
        if let Some(cell) = frame.buffer_mut().cell_mut((area.x, y))
            && cell.symbol().contains('\u{1b}')
        {
            cell.set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(
                std::num::NonZeroU16::MIN,
            ));
        }
    }
}

/// Upscale `img` so it fills at least `area` terminal cells (using `font_size` px/cell).
/// If the image is already large enough, returns a clone unchanged.
/// This ensures protocol renderers (Kitty/iTerm2) don't render at native pixel size
/// when the thumbnail API returned a low-resolution image.
pub(super) fn upscale_for_rect(
    img: &image::DynamicImage,
    area: Rect,
    font_size: (u16, u16),
) -> image::DynamicImage {
    let target_px_w = area.width as u32 * font_size.0 as u32;
    let target_px_h = area.height as u32 * font_size.1 as u32;
    if target_px_w == 0 || target_px_h == 0 {
        return img.clone();
    }
    if img.width() < target_px_w || img.height() < target_px_h {
        img.resize(
            target_px_w,
            target_px_h,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img.clone()
    }
}

/// Compute a horizontally-centered sub-rect for a protocol image inside `area`.
/// When the image is portrait (height-constrained), it renders narrower than
/// `area.width`; this centers it so the text below is unaffected.
pub(super) fn center_image_rect(img: &image::DynamicImage, area: Rect) -> Rect {
    use image::GenericImageView;
    let (orig_w, orig_h) = img.dimensions();
    if orig_h == 0 || area.height == 0 {
        return area;
    }
    let orig_aspect = orig_w as f32 / orig_h as f32;
    let natural_w = (area.height as f32 * 2.0 * orig_aspect) as u16;
    if natural_w >= area.width {
        // Landscape / square: already fills full width, no shift needed
        return area;
    }
    // Portrait: narrower than the panel — center horizontally
    Rect {
        x: area.x + (area.width - natural_w) / 2,
        y: area.y,
        width: natural_w,
        height: area.height,
    }
}

/// Render image to colored halfblock lines.
pub(super) fn render_image_to_colored_lines(
    img: &image::DynamicImage,
    max_width: u32,
    max_height: u32,
) -> Vec<Line<'static>> {
    use image::GenericImageView;

    let (orig_w, orig_h) = img.dimensions();
    if orig_w == 0 || orig_h == 0 || max_width == 0 || max_height == 0 {
        return vec![];
    }
    let orig_aspect = orig_w as f32 / orig_h as f32;

    // Terminal characters are ~2x taller than wide
    let target_width = max_width;
    let target_height_chars = ((target_width as f32 / orig_aspect) / 2.0).max(1.0) as u32;

    let (final_width, final_height_chars) = if target_height_chars > max_height {
        let h = max_height;
        let w = ((h as f32 * 2.0 * orig_aspect) as u32).max(1);
        (w, h)
    } else {
        (target_width, target_height_chars)
    };

    let left_pad = (max_width.saturating_sub(final_width) / 2) as usize;

    // Resize to double height (each char shows 2 pixels vertically)
    let img = img.resize(
        final_width,
        final_height_chars * 2,
        image::imageops::FilterType::Lanczos3,
    );

    let (w, h) = img.dimensions();
    let mut lines = Vec::new();

    for y in 0..final_height_chars {
        let mut spans = Vec::new();
        if left_pad > 0 {
            spans.push(Span::raw(" ".repeat(left_pad)));
        }
        for x in 0..w {
            let y_top = (y * 2).min(h - 1);
            let y_bottom = (y * 2 + 1).min(h - 1);

            let top_pixel = img.get_pixel(x, y_top);
            let bottom_pixel = img.get_pixel(x, y_bottom);

            let span = Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(top_pixel[0], top_pixel[1], top_pixel[2]))
                    .bg(Color::Rgb(
                        bottom_pixel[0],
                        bottom_pixel[1],
                        bottom_pixel[2],
                    )),
            );
            spans.push(span);
        }
        lines.push(Line::from(spans));
    }

    lines
}

/// Render image to grayscale ASCII art lines.
pub(super) fn render_image_to_grayscale_lines(
    img: &image::DynamicImage,
    max_width: u32,
    max_height: u32,
) -> Vec<Line<'static>> {
    use image::GenericImageView;

    const ASCII_CHARS: &[char] = &[' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

    let (orig_w, orig_h) = img.dimensions();
    if orig_w == 0 || orig_h == 0 || max_width == 0 || max_height == 0 {
        return vec![];
    }
    let orig_aspect = orig_w as f32 / orig_h as f32;

    // Terminal characters are ~2x taller than wide
    let target_width = max_width;
    let target_height_chars = ((target_width as f32 / orig_aspect) / 2.0).max(1.0) as u32;

    let (final_width, final_height_chars) = if target_height_chars > max_height {
        let h = max_height;
        let w = ((h as f32 * 2.0 * orig_aspect) as u32).max(1);
        (w, h)
    } else {
        (target_width, target_height_chars)
    };

    let left_pad = (max_width.saturating_sub(final_width) / 2) as usize;

    // Resize to double height (sample every 2 rows)
    let img = img.resize(
        final_width,
        final_height_chars * 2,
        image::imageops::FilterType::Lanczos3,
    );

    let (w, h) = img.dimensions();
    let mut lines = Vec::new();

    for y in 0..final_height_chars {
        let mut line_str = if left_pad > 0 {
            " ".repeat(left_pad)
        } else {
            String::new()
        };
        for x in 0..w {
            let y1 = (y * 2).min(h - 1);
            let y2 = (y * 2 + 1).min(h - 1);

            let pixel1 = img.get_pixel(x, y1);
            let pixel2 = img.get_pixel(x, y2);

            let brightness = ((pixel1[0] as u32 + pixel2[0] as u32) / 2
                + (pixel1[1] as u32 + pixel2[1] as u32) / 2
                + (pixel1[2] as u32 + pixel2[2] as u32) / 2)
                / 3;

            let idx = (brightness as usize * ASCII_CHARS.len()) / 256;
            line_str.push(ASCII_CHARS[idx.min(ASCII_CHARS.len() - 1)]);
        }
        lines.push(Line::from(line_str));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, widgets::Paragraph};
    use ratatui_image::picker::{Picker, ProtocolType};

    #[test]
    fn native_protocols_preserve_neighboring_text_and_image_transmissions() {
        for protocol_type in [
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
            ProtocolType::Sixel,
        ] {
            let mut picker = Picker::halfblocks();
            picker.set_protocol_type(protocol_type);
            let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                16,
                16,
                image::Rgba([80, 120, 180, 255]),
            ));
            let mut first = picker.new_resize_protocol(image.clone());
            let mut second = picker.new_resize_protocol(image);
            let mut terminal = Terminal::new(TestBackend::new(12, 3)).unwrap();
            terminal
                .draw(|frame| {
                    frame.render_widget(
                        Paragraph::new("   A     B\n   C     D\nfooter"),
                        frame.area(),
                    );
                    render_image_protocol(frame, Rect::new(1, 0, 2, 2), &mut first);
                    render_image_protocol(frame, Rect::new(7, 0, 2, 2), &mut second);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            for (position, expected) in [
                ((3, 0), "A"),
                ((9, 0), "B"),
                ((3, 1), "C"),
                ((9, 1), "D"),
                ((0, 2), "f"),
            ] {
                assert_eq!(buffer[position].symbol(), expected, "{protocol_type:?}");
            }
            for x in [1, 7] {
                let payload = buffer[(x, 0)].symbol();
                assert!(payload.contains('\u{1b}'), "{protocol_type:?}, x={x}");
                if protocol_type == ProtocolType::Kitty {
                    assert!(
                        payload.contains("a=T"),
                        "each Kitty image must transmit its pixels on its first frame"
                    );
                }
            }
        }
    }
}
