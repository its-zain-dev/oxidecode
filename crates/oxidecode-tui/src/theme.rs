//! OxideCode theme.
//!
//! These are the exact colours from OpenCode's `v2/opencode` theme, resolved
//! from `theme/assets/v2/opencode.json`: neutral greys for text, orange for
//! interactive elements and purple for accents.

use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub background_raised: Color,
    pub background_element: Color,
    pub background_menu: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub border: Color,
    pub border_subtle: Color,
    pub border_active: Color,
    pub accent: Color,
    pub interactive: Color,
    pub secondary: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub info: Color,
    pub purple: Color,
    pub blue: Color,
    pub cyan: Color,
    pub green: Color,
    pub orange: Color,
    pub red: Color,
    pub yellow: Color,
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_context: Color,
    pub diff_added_bg: Color,
    pub diff_removed_bg: Color,
    pub diff_added_line_bg: Color,
    pub diff_removed_line_bg: Color,
    pub markdown_heading: Color,
    pub markdown_link: Color,
    pub markdown_link_text: Color,
    pub markdown_code: Color,
    pub markdown_quote: Color,
    pub markdown_emphasis: Color,
    pub markdown_strong: Color,
    pub markdown_rule: Color,
    pub markdown_list: Color,
    pub markdown_enum: Color,
    pub syntax_comment: Color,
    pub syntax_keyword: Color,
    pub syntax_function: Color,
    pub syntax_variable: Color,
    pub syntax_string: Color,
    pub syntax_number: Color,
    pub syntax_type: Color,
    pub syntax_operator: Color,
    pub syntax_punctuation: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: rgb(0x0a, 0x0a, 0x0a),
            background_raised: rgb(0x14, 0x14, 0x14),
            background_element: rgb(0x1e, 0x1e, 0x1e),
            background_menu: rgb(0x1e, 0x1e, 0x1e),
            text: rgb(0xee, 0xee, 0xee),
            muted: rgb(0x80, 0x80, 0x80),
            faint: rgb(0x4c, 0x4c, 0x4c),
            border: rgb(0x48, 0x48, 0x48),
            border_subtle: rgb(0x1e, 0x1e, 0x1e),
            border_active: rgb(0xb5, 0xb5, 0xb5),
            accent: rgb(0x9d, 0x7c, 0xd8),
            interactive: rgb(0xfa, 0xb2, 0x83),
            secondary: rgb(0x56, 0xb6, 0xc2),
            success: rgb(0x7f, 0xd8, 0x8f),
            error: rgb(0xe0, 0x6c, 0x75),
            warning: rgb(0xf5, 0xa7, 0x42),
            info: rgb(0x56, 0xb6, 0xc2),
            purple: rgb(0x9d, 0x7c, 0xd8),
            blue: rgb(0x5c, 0x9c, 0xf5),
            cyan: rgb(0x56, 0xb6, 0xc2),
            green: rgb(0x7f, 0xd8, 0x8f),
            orange: rgb(0xfa, 0xb2, 0x83),
            red: rgb(0xe0, 0x6c, 0x75),
            yellow: rgb(0xe5, 0xc0, 0x7b),
            diff_added: rgb(0x4f, 0xd6, 0xbe),
            diff_removed: rgb(0xc5, 0x3b, 0x53),
            diff_context: rgb(0x82, 0x8b, 0xb8),
            diff_added_bg: rgb(0x20, 0x30, 0x3b),
            diff_removed_bg: rgb(0x37, 0x22, 0x2c),
            diff_added_line_bg: rgb(0x1b, 0x2b, 0x34),
            diff_removed_line_bg: rgb(0x2d, 0x1f, 0x26),
            markdown_heading: rgb(0x9d, 0x7c, 0xd8),
            markdown_link: rgb(0xfa, 0xb2, 0x83),
            markdown_link_text: rgb(0x56, 0xb6, 0xc2),
            markdown_code: rgb(0x7f, 0xd8, 0x8f),
            markdown_quote: rgb(0xe5, 0xc0, 0x7b),
            markdown_emphasis: rgb(0xe5, 0xc0, 0x7b),
            markdown_strong: rgb(0xf5, 0xa7, 0x42),
            markdown_rule: rgb(0x80, 0x80, 0x80),
            markdown_list: rgb(0xfa, 0xb2, 0x83),
            markdown_enum: rgb(0x56, 0xb6, 0xc2),
            syntax_comment: rgb(0x80, 0x80, 0x80),
            syntax_keyword: rgb(0x9d, 0x7c, 0xd8),
            syntax_function: rgb(0xfa, 0xb2, 0x83),
            syntax_variable: rgb(0xe0, 0x6c, 0x75),
            syntax_string: rgb(0x7f, 0xd8, 0x8f),
            syntax_number: rgb(0xf5, 0xa7, 0x42),
            syntax_type: rgb(0xe5, 0xc0, 0x7b),
            syntax_operator: rgb(0x56, 0xb6, 0xc2),
            syntax_punctuation: rgb(0xee, 0xee, 0xee),
        }
    }
}

impl Theme {
    /// Blends `fg` into `bg` with the given alpha (0 = background, 1 = foreground).
    pub fn tint(&self, bg: Color, fg: Color, alpha: f32) -> Color {
        let (br, bg_g, bb) = components(bg);
        let (fr, fg_g, fb) = components(fg);
        let mix = |a: u8, b: u8| -> u8 {
            (a as f32 * (1.0 - alpha) + b as f32 * alpha).round().clamp(0.0, 255.0) as u8
        };
        Color::Rgb(
            mix(br, fr),
            mix(bg_g, fg_g),
            mix(bb, fb),
        )
    }

    pub fn shadow(&self, fg: Color) -> Color {
        self.tint(self.background, fg, 0.25)
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

fn components(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        Color::Gray => (128, 128, 128),
        Color::DarkGray => (64, 64, 64),
        Color::Red => (255, 0, 0),
        Color::Green => (0, 255, 0),
        Color::Yellow => (255, 255, 0),
        Color::Blue => (0, 0, 255),
        Color::Magenta => (255, 0, 255),
        Color::Cyan => (0, 255, 255),
        _ => (0, 0, 0),
    }
}
