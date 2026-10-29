use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Full logical lines plus width-specific visual rows. Only visible rows are rendered,
/// so history is not limited by Paragraph's u16 scroll offset.
#[derive(Default)]
pub(super) struct OutputHistory {
    lines: Vec<String>,
    rows: Vec<String>,
    width: usize,
    height: usize,
    offset: usize,
    manually_scrolled: bool,
}
impl OutputHistory {
    pub fn push(&mut self, text: String) {
        for line in text.split('\n') {
            // Control characters must not influence terminal state or row measurement.
            let line: String = line
                .chars()
                .filter(|c| !c.is_control() || *c == '\t')
                .collect::<String>()
                .replace('\t', "    ");
            self.rows.extend(wrap(&line, self.width.max(1)));
            self.lines.push(line);
        }
        self.clamp();
    }
    pub fn resize(&mut self, width: usize, height: usize) {
        if self.width != width.max(1) {
            self.width = width.max(1);
            self.rows = self
                .lines
                .iter()
                .flat_map(|line| wrap(line, self.width))
                .collect();
        }
        self.height = height;
        self.clamp();
    }
    fn clamp(&mut self) {
        self.offset = if self.manually_scrolled {
            self.offset.min(self.max_offset())
        } else {
            self.max_offset()
        };
    }
    fn max_offset(&self) -> usize {
        self.rows.len().saturating_sub(self.height)
    }
    pub fn up(&mut self, rows: usize) {
        self.manually_scrolled = true;
        self.offset = self.offset.saturating_sub(rows);
    }
    pub fn down(&mut self, rows: usize) {
        self.manually_scrolled = true;
        self.offset = self.offset.saturating_add(rows).min(self.max_offset());
    }
    pub fn home(&mut self) {
        self.manually_scrolled = true;
        self.offset = 0;
    }
    pub fn follow(&mut self) {
        self.manually_scrolled = false;
        self.clamp();
    }
    pub fn following(&self) -> bool {
        !self.manually_scrolled
    }
    pub fn offset(&self) -> usize {
        self.offset
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn visible(&self) -> &[String] {
        &self.rows[self.offset..self.offset.saturating_add(self.height).min(self.rows.len())]
    }
}
fn wrap(line: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    for grapheme in line.graphemes(true) {
        let size = grapheme.width();
        if used + size > width && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        row.push_str(grapheme);
        used += size;
    }
    rows.push(row);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_history_is_kept_and_tail_follows_until_user_scrolls() {
        let mut h = OutputHistory::default();
        h.resize(40, 3);
        for i in 0..200 {
            h.push(format!("caption {i}"));
        }
        assert_eq!(h.len(), 200);
        assert_eq!(h.visible(), ["caption 197", "caption 198", "caption 199"]);
        h.up(2);
        let before = h.visible().to_vec();
        h.push("caption 200".into());
        assert_eq!(h.visible(), before);
        h.home();
        assert_eq!(h.visible()[0], "caption 0");
        h.down(usize::MAX);
        assert!(!h.following());
        h.push("caption 201".into());
        assert_eq!(h.visible().last().unwrap(), "caption 200");
        h.follow();
        assert_eq!(h.visible().last().unwrap(), "caption 201");
        h.push("caption 202".into());
        assert_eq!(h.visible().last().unwrap(), "caption 202");
    }
    #[test]
    fn wrap_handles_long_korean_and_graphemes_and_resize() {
        let mut h = OutputHistory::default();
        h.resize(4, 2);
        h.push("한국어 자막".into());
        assert_eq!(h.rows, ["한국", "어 ", "자막"]);
        h.push("e\u{301}👩‍💻".into());
        assert_eq!(h.rows.last().unwrap(), "e\u{301}👩‍💻");
        h.resize(80, 10);
        assert_eq!(h.visible(), ["한국어 자막", "e\u{301}👩‍💻"]);
        h.resize(2, 1);
        assert_eq!(h.visible().last().unwrap(), "👩‍💻");
        h.resize(0, 0);
        assert!(h.visible().is_empty());
    }
    #[test]
    fn history_can_scroll_past_u16_rows_and_long_text_is_not_truncated() {
        let mut h = OutputHistory::default();
        h.resize(1000, 2);
        h.push("한".repeat(400));
        assert_eq!(h.visible()[0].chars().count(), 400);
        for i in 0..70_000 {
            h.push(i.to_string());
        }
        assert_eq!(h.offset(), 69_999);
        assert_eq!(h.visible(), ["69998", "69999"]);
        h.home();
        assert_eq!(h.visible()[0].chars().count(), 400);
    }
}
