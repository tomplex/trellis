// torchard-rs/src/tui/theme.rs

use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(0x1a, 0x1a, 0x2e);
pub const HEADER_BG: Color = Color::Rgb(0x16, 0x21, 0x3e);
pub const ACCENT: Color = Color::Rgb(0x00, 0xaa, 0xff);
pub const CURSOR_BG: Color = Color::Rgb(0x0f, 0x34, 0x60);
pub const TEXT: Color = Color::Rgb(0xe0, 0xe0, 0xe0);
pub const TEXT_DIM: Color = Color::Rgb(0xaa, 0xaa, 0xaa);
pub const RED: Color = Color::Rgb(0xff, 0x6b, 0x6b);
pub const GREEN: Color = Color::Rgb(0x51, 0xcf, 0x66);
pub const YELLOW: Color = Color::Rgb(0xff, 0xd4, 0x3b);
pub const ORANGE: Color = Color::Rgb(0xe8, 0x7b, 0x35);
pub const PURPLE: Color = Color::Rgb(0xcc, 0x5d, 0xe8);
pub const CYAN: Color = Color::Rgb(0x22, 0xb8, 0xcf);
pub const PINK: Color = Color::Rgb(0xf0, 0x65, 0x95);
pub const BLUE: Color = Color::Rgb(0x00, 0xaa, 0xff);

pub const REPO_COLORS: [Color; 8] = [
    BLUE,
    RED,
    GREEN,
    YELLOW,
    PURPLE,
    Color::Rgb(0xff, 0x92, 0x2b), // orange
    CYAN,
    PINK,
];

pub fn style_default() -> Style {
    Style::default().fg(TEXT).bg(BG)
}

pub fn style_header() -> Style {
    Style::default().fg(ACCENT).bg(HEADER_BG).add_modifier(Modifier::BOLD)
}

pub fn style_cursor() -> Style {
    Style::default().fg(Color::White).bg(CURSOR_BG)
}

pub fn style_footer() -> Style {
    Style::default().fg(TEXT_DIM).bg(HEADER_BG)
}

pub fn style_footer_key() -> Style {
    Style::default().fg(ACCENT).bg(HEADER_BG).add_modifier(Modifier::BOLD)
}

pub fn style_dim() -> Style {
    Style::default().fg(TEXT_DIM)
}

pub fn style_error() -> Style {
    Style::default().fg(RED)
}

pub fn style_green() -> Style {
    Style::default().fg(GREEN)
}
