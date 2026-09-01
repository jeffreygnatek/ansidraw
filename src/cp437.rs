//! Code page 437 <-> Unicode mapping. .ANS files are CP437 bytes; the canvas
//! stores Unicode chars for display and converts at load/save time.

/// Glyphs for bytes 0x00-0x1F (0x00 rendered as space).
const LOW: [char; 32] = [
    ' ', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼',
    '►', '◄', '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
];

/// Glyphs for bytes 0x80-0xFF.
const HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{00A0}',
];

pub fn byte_to_char(b: u8) -> char {
    match b {
        0x00..=0x1F => LOW[b as usize],
        0x20..=0x7E => b as char,
        0x7F => '⌂',
        _ => HIGH[(b - 0x80) as usize],
    }
}

pub fn char_to_byte(c: char) -> u8 {
    if (' '..='~').contains(&c) {
        return c as u8;
    }
    if c == '⌂' {
        return 0x7F;
    }
    if let Some(i) = HIGH.iter().position(|&h| h == c) {
        return 0x80 + i as u8;
    }
    if let Some(i) = LOW.iter().position(|&l| l == c) {
        if i != 0 {
            return i as u8;
        }
    }
    b'?'
}
