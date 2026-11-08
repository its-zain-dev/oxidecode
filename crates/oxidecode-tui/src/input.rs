//! A small multiline text editor for the prompt.
//!
//! The editor stores chars (not bytes) so cursor movement is trivially safe for
//! UTF-8. Visual wrapping is computed from terminal cell widths so the cursor
//! and viewport stay correct for CJK and emoji.

use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Default)]
pub struct Input {
    pub chars: Vec<char>,
    pub cursor: usize,
    preferred_col: Option<usize>,
    history: Vec<String>,
    history_index: Option<usize>,
    stash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VisualLine {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub width: usize,
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn set_text(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
        self.preferred_col = None;
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.preferred_col = None;
        self.history_index = None;
        self.stash = None;
    }

    pub fn take(&mut self) -> String {
        let text = self.text();
        let trimmed = text.trim_end().to_string();
        self.clear();
        if !trimmed.is_empty() {
            self.history.push(trimmed.clone());
        }
        trimmed
    }

    pub fn insert_char(&mut self, ch: char) {
        if ch == '\r' {
            return;
        }
        self.chars.insert(self.cursor, ch);
        self.cursor += 1;
        self.preferred_col = None;
        self.history_index = None;
    }

    pub fn insert_str(&mut self, text: &str) {
        let clean = text.replace("\r\n", "\n").replace('\r', "\n");
        for ch in clean.chars() {
            self.chars.insert(self.cursor, ch);
            self.cursor += 1;
        }
        self.preferred_col = None;
        self.history_index = None;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            self.preferred_col = None;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
            self.preferred_col = None;
        }
    }

    pub fn delete_word_back(&mut self) {
        let mut index = self.cursor;
        while index > 0 && self.chars[index - 1].is_whitespace() {
            index -= 1;
        }
        while index > 0 && !self.chars[index - 1].is_whitespace() {
            index -= 1;
        }
        self.chars.drain(index..self.cursor);
        self.cursor = index;
        self.preferred_col = None;
    }

    pub fn delete_line(&mut self) {
        let start = self.line_start(self.cursor);
        let end = self.line_end(self.cursor);
        self.chars.drain(start..end);
        self.cursor = start;
        self.preferred_col = None;
    }

    pub fn kill_to_end(&mut self) {
        let end = self.line_end(self.cursor);
        self.chars.drain(self.cursor..end);
        self.preferred_col = None;
    }

    pub fn left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.preferred_col = None;
        }
    }

    pub fn right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
            self.preferred_col = None;
        }
    }

    pub fn word_left(&mut self) {
        let mut index = self.cursor;
        while index > 0 && self.chars[index - 1].is_whitespace() {
            index -= 1;
        }
        while index > 0 && !self.chars[index - 1].is_whitespace() {
            index -= 1;
        }
        self.cursor = index;
        self.preferred_col = None;
    }

    pub fn word_right(&mut self) {
        let mut index = self.cursor;
        while index < self.chars.len() && !self.chars[index].is_whitespace() {
            index += 1;
        }
        while index < self.chars.len() && self.chars[index].is_whitespace() {
            index += 1;
        }
        self.cursor = index;
        self.preferred_col = None;
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
        self.preferred_col = None;
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
        self.preferred_col = None;
    }

    fn line_start(&self, index: usize) -> usize {
        let index = index.min(self.chars.len());
        self.chars[..index]
            .iter()
            .rposition(|ch| *ch == '\n')
            .map(|position| position + 1)
            .unwrap_or(0)
    }

    fn line_end(&self, index: usize) -> usize {
        let index = index.min(self.chars.len());
        self.chars[index..]
            .iter()
            .position(|ch| *ch == '\n')
            .map(|position| index + position)
            .unwrap_or(self.chars.len())
    }

    pub fn up(&mut self, width: usize) {
        let width = width.max(1);
        let lines = self.visual_lines(width);
        let (row, col) = self.visual_position(&lines, width);
        if row == 0 {
            return;
        }
        let preferred = self.preferred_col.unwrap_or(col);
        let target = &lines[row - 1];
        let offset = preferred.min(target.width);
        self.cursor = target.start + char_at_width(&self.chars[target.start..target.end], offset);
        self.preferred_col = Some(preferred);
    }

    pub fn down(&mut self, width: usize) {
        let width = width.max(1);
        let lines = self.visual_lines(width);
        let (row, col) = self.visual_position(&lines, width);
        if row + 1 >= lines.len() {
            return;
        }
        let preferred = self.preferred_col.unwrap_or(col);
        let target = &lines[row + 1];
        let offset = preferred.min(target.width);
        self.cursor = target.start + char_at_width(&self.chars[target.start..target.end], offset);
        self.preferred_col = Some(preferred);
    }

    pub fn visual_lines(&self, width: usize) -> Vec<VisualLine> {
        let width = width.max(1);
        let mut out = Vec::new();
        let mut logical_start = 0usize;
        let mut index = 0usize;
        while index <= self.chars.len() {
            let at_end = index == self.chars.len();
            let newline = !at_end && self.chars[index] == '\n';
            if at_end || newline {
                let end = index;
                if end == logical_start {
                    out.push(VisualLine {
                        start: logical_start,
                        end,
                        text: String::new(),
                        width: 0,
                    });
                } else {
                    let mut start = logical_start;
                    while start < end {
                        let mut cells = 0usize;
                        let mut cursor = start;
                        while cursor < end {
                            let ch_width = self.chars[cursor].width().unwrap_or(1).max(1);
                            if cells + ch_width > width && cursor > start {
                                break;
                            }
                            cells += ch_width;
                            cursor += 1;
                        }
                        let text: String = self.chars[start..cursor].iter().collect();
                        out.push(VisualLine {
                            start,
                            end: cursor,
                            text,
                            width: cells,
                        });
                        start = cursor;
                    }
                }
                logical_start = index + 1;
                if at_end {
                    break;
                }
            }
            index += 1;
        }
        out
    }

    pub fn visual_position(&self, lines: &[VisualLine], width: usize) -> (usize, usize) {
        let _ = width;
        if lines.is_empty() {
            return (0, 0);
        }
        for (row, line) in lines.iter().enumerate() {
            if self.cursor >= line.start && self.cursor < line.end {
                return (row, display_width(&self.chars[line.start..self.cursor]));
            }
            if self.cursor == line.end {
                let next_has_cursor = lines
                    .get(row + 1)
                    .map(|next| next.start == line.end)
                    .unwrap_or(false);
                if !next_has_cursor {
                    return (row, line.width);
                }
            }
        }
        let last = lines.len() - 1;
        (last, lines[last].width)
    }

    pub fn cursor_position(&self, width: usize) -> (usize, usize) {
        let lines = self.visual_lines(width);
        self.visual_position(&lines, width)
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None => {
                self.stash = Some(self.text());
                self.history.len() - 1
            }
        };
        self.history_index = Some(next);
        let text = self.history[next].clone();
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
        self.preferred_col = None;
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            self.history_index = Some(index + 1);
            let text = self.history[index + 1].clone();
            self.chars = text.chars().collect();
            self.cursor = self.chars.len();
        } else {
            self.history_index = None;
            let text = self.stash.take().unwrap_or_default();
            self.chars = text.chars().collect();
            self.cursor = self.chars.len();
        }
        self.preferred_col = None;
    }
}

fn display_width(chars: &[char]) -> usize {
    chars
        .iter()
        .map(|ch| ch.width().unwrap_or(1).max(1))
        .sum()
}

fn char_at_width(chars: &[char], target: usize) -> usize {
    let mut width = 0usize;
    for (index, ch) in chars.iter().enumerate() {
        if width >= target {
            return index;
        }
        width += ch.width().unwrap_or(1).max(1);
    }
    chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_tracks_cursor() {
        let mut input = Input::new();
        input.insert_str("hello world");
        let lines = input.visual_lines(5);
        assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), vec!["hello", " worl", "d"]);
        input.cursor = 8;
        assert_eq!(input.cursor_position(5), (1, 3));
    }

    #[test]
    fn respects_newlines() {
        let mut input = Input::new();
        input.insert_str("a\n\nb");
        let lines = input.visual_lines(10);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].text, "");
    }
}
