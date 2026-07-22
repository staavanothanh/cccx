use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::live_wrap::take_prefix_by_width;
use crate::render::renderable::Renderable;

#[derive(Debug, Default)]
pub(crate) struct CccxWorkerFooter {
    rows: Vec<String>,
}

impl CccxWorkerFooter {
    pub(crate) fn set_rows(&mut self, mut rows: Vec<String>) -> bool {
        const MAX_VISIBLE_WORKERS: usize = 8;
        if rows.len() > MAX_VISIBLE_WORKERS {
            let hidden = rows.len() - MAX_VISIBLE_WORKERS;
            rows.truncate(MAX_VISIBLE_WORKERS);
            rows.push(format!(
                "+{hidden} more active/resumable worker(s) · /agents list"
            ));
        }
        if self.rows == rows {
            return false;
        }
        self.rows = rows;
        true
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width < 4 {
            return Vec::new();
        }
        self.rows
            .iter()
            .map(|row| {
                let line = format!("  └─ ● {row}");
                let (truncated, _, _) = take_prefix_by_width(&line, width as usize);
                Line::from(truncated)
            })
            .collect()
    }
}

impl Renderable for CccxWorkerFooter {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        Paragraph::new(self.render_lines(area.width)).render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cccx_worker_footer_keeps_text_identity_when_colors_would_repeat() {
        let mut footer = CccxWorkerFooter::default();
        footer.set_rows(
            (1..=9)
                .map(|index| {
                    format!(
                        "agent-{index} logical:session-{index} CX implementer \"task {index}\" [running]"
                    )
                })
                .collect(),
        );
        let lines = footer.render_lines(120);
        assert_eq!(lines.len(), 9, "eight identities plus one overflow summary");
        for (index, line) in lines.iter().take(8).enumerate() {
            let rendered = line.to_string();
            assert!(rendered.contains(&format!("agent-{}", index + 1)));
            assert!(rendered.contains(&format!("logical:session-{}", index + 1)));
            assert!(rendered.contains("[running]"));
        }
        assert!(lines[8].to_string().contains("+1 more"));
        assert!(lines[8].to_string().contains("/agents list"));
    }
}
