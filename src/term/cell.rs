pub const BOLD: u16 = 1;
pub const DIM: u16 = 2;
pub const ITALIC: u16 = 4;
pub const UNDERLINE: u16 = 8;
pub const BLINK: u16 = 16;
pub const REVERSE: u16 = 32;
pub const HIDDEN: u16 = 64;
pub const STRIKE: u16 = 128;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub attrs: u16,
}

/// The text that stands for a picture when cells are turned into text (`image-copy-text`).
pub const DEFAULT_IMAGE_TEXT: &str = "[image]";

thread_local! {
    static IMAGE_TEXT: std::cell::RefCell<String> = std::cell::RefCell::new(DEFAULT_IMAGE_TEXT.to_string());
}

/// Set the text for a picture; the server calls this when `image-copy-text` changes.
pub fn set_image_text(text: &str) {
    IMAGE_TEXT.with(|t| *t.borrow_mut() = text.to_string());
}

/// `width`: 1 normal, 2 lead half of a wide char, 0 trailing half of a wide char.
/// `tile`: index of an image tile (0 = none).
/// `mark`: index of the zero-width characters that follow `ch` (0 = none, see `marks`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub width: u8,
    pub style: Style,
    pub tile: u32,
    pub mark: u16,
}

impl Cell {
    pub const BLANK: Cell =
        Cell { ch: ' ', width: 1, style: Style { fg: Color::Default, bg: Color::Default, attrs: 0 }, tile: 0, mark: 0 };

    /// The character and its marks as text.
    pub fn push_to(&self, out: &mut String) {
        out.push(self.ch);
        super::marks::push_to(out, self.mark);
    }

    /// The text of some cells: the trailing halves of wide characters left out, a NUL as a space,
    /// the marks after their character. A picture cell is a space.
    #[cfg(test)]
    pub fn text_of<'a>(cells: impl IntoIterator<Item = &'a Cell>) -> String {
        Cell::text_with(cells, false)
    }

    /// `text_of` for text that leaves the screen (copy mode, `capture-pane`, the agent preview):
    /// a run of picture cells is the text of `image-copy-text`, once for the run, nothing when that
    /// is empty.
    pub fn text_for_copy<'a>(cells: impl IntoIterator<Item = &'a Cell>) -> String {
        Cell::text_with(cells, true)
    }

    fn text_with<'a>(cells: impl IntoIterator<Item = &'a Cell>, pictures: bool) -> String {
        let mut s = String::new();
        let mut in_picture = false;
        for c in cells.into_iter().filter(|c| c.width != 0) {
            if pictures && c.tile != 0 {
                if !in_picture {
                    IMAGE_TEXT.with(|t| s.push_str(&t.borrow()));
                }
                in_picture = true;
                continue;
            }
            in_picture = false;
            if c.ch == '\0' {
                s.push(' ');
            } else {
                c.push_to(&mut s);
            }
        }
        s
    }

    /// Cell left behind by erase/scroll: keeps only the pen's background (BCE).
    pub fn erased(pen: Style) -> Cell {
        Cell { style: Style { fg: Color::Default, bg: pen.bg, attrs: 0 }, ..Cell::BLANK }
    }

    pub fn is_blank(&self) -> bool {
        *self == Cell::BLANK
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// This row continues onto the next one (soft wrap), used by reflow and copy mode.
    pub wrapped: bool,
    /// Cells of an image row that a narrower pane cut off, kept so widening it again shows the
    /// whole image. Starts at the column after the last visible one; dropped when the row is written.
    pub clipped: Vec<Cell>,
}

impl Row {
    pub fn new(cols: usize, fill: Cell) -> Row {
        Row { cells: vec![fill; cols], wrapped: false, clipped: Vec::new() }
    }

    /// Drop trailing default blanks; scrollback rows are stored this way to save memory.
    pub fn trim(&mut self) {
        if !self.clipped.is_empty() {
            return;
        }
        while self.cells.last().is_some_and(Cell::is_blank) {
            self.cells.pop();
        }
    }

    pub fn pad_to(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::BLANK);
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        let mut s = Cell::text_of(&self.cells);
        s.truncate(s.trim_end().len());
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(spec: &str) -> Vec<Cell> {
        spec.chars().map(|ch| if ch == '#' { Cell { tile: 1, ..Cell::BLANK } } else { Cell { ch, ..Cell::BLANK } }).collect()
    }

    #[test]
    fn a_run_of_picture_cells_is_the_picture_text_once() {
        assert_eq!(Cell::text_for_copy(&cells("ab###cd")), "ab[image]cd");
        assert_eq!(Cell::text_for_copy(&cells("#a#")), "[image]a[image]");
    }

    #[test]
    fn the_picture_text_can_be_changed_or_empty() {
        set_image_text("");
        assert_eq!(Cell::text_for_copy(&cells("ab###cd")), "abcd");
        set_image_text("<pic>");
        assert_eq!(Cell::text_for_copy(&cells("###")), "<pic>");
        set_image_text(DEFAULT_IMAGE_TEXT);
    }

    #[test]
    fn text_without_pictures_is_unchanged_and_the_screen_text_keeps_spaces() {
        assert_eq!(Cell::text_for_copy(&cells("plain text")), "plain text");
        assert_eq!(Cell::text_of(&cells("a##b")), "a  b");
    }
}
