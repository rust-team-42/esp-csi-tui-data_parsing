use std::collections::btree_map::Values;

use ratatui::{
    prelude::Buffer,
    prelude::Rect,
    style::{Color, Style},
    widgets::{Widget},
};

#[derive(Debug, Clone)]
pub struct Heatmap {
    pub values: Vec<Vec<u8>>, // 0–100 values
}

impl Widget for &Heatmap {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let rows = self.values.len();
        if rows == 0 {
            return;
        }
        let cols = self.values[0].len();

        // Keep within terminal bounds
        let height = rows.min(area.height as usize);
        let width = cols.min(area.width as usize);

        for y in 0..height {
            for x in 0..width {
                let value = self.values[y][x];


                let color = heatmap_color(value);
                // Draw a block (two spaces to make it square-ish)
                let symbol = "  ";

                buf.set_string(
                    area.x + x as u16,
                    area.y + y as u16,
                    symbol,
                    Style::default().bg(color),
                );
            }
        }
    }
}

fn heatmap_color(value: u8) -> Color {
    // Clamp to 0–100
    let v = value.min(100);

    // Bucket into ranges of size 2: 0..=1, 2..=3, ..., 98..=100
    let bucket = (v / 2) * 2;          // 0, 2, 4, ..., 100
    let t = bucket as f32 / 100.0;     // 0.0 .. 1.0

    // t = 0.0  -> warm (orange/yellow)
    // t = 1.0  -> cold (blue)
    let r = (255.0 * t) as u8;   // fades from 255 → 0
    let g = (200.0 * t) as u8;   // fades from 200 → 0
    let b = (255.0 * (1.0 - t)) as u8;           // grows from 0   → 255

    Color::Rgb(r, g, b)
}