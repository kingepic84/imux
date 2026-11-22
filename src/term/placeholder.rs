//! Kitty unicode placeholders (`U=1`). A program that cannot count on the terminal to put a
//! picture where the cursor is (it runs inside a multiplexer, or scrolls the text itself) sends
//! the picture once with `U=1` and then prints the character U+10EEEE wherever a piece of it
//! should show. The cell's foreground color is the image id (24 bits, the top 8 bits optionally
//! in a third diacritic), and up to two combining marks after the character give the row and the
//! column of the piece, as an index into a fixed list of diacritics. Without them the piece
//! follows the one to its left.
//!
//! The list is the one of the protocol: the 297 combining marks of class "above" that no letter
//! composes with, as of Unicode 6.0 (it must never change, since programs index into it).

use super::cell::Color;
use super::grid::Grid;
use super::marks;
use crate::image::store::ImageId;

pub const PLACEHOLDER: char = '\u{10EEEE}';

#[rustfmt::skip]
const DIACRITICS: [u32; 297] = [
    0x305, 0x30D, 0x30E, 0x310, 0x312, 0x33D, 0x33E, 0x33F, 0x346, 0x34A, 0x34B, 0x34C,
    0x350, 0x351, 0x352, 0x357, 0x35B, 0x363, 0x364, 0x365, 0x366, 0x367, 0x368, 0x369,
    0x36A, 0x36B, 0x36C, 0x36D, 0x36E, 0x36F, 0x483, 0x484, 0x485, 0x486, 0x487, 0x592,
    0x593, 0x594, 0x595, 0x597, 0x598, 0x599, 0x59C, 0x59D, 0x59E, 0x59F, 0x5A0, 0x5A1,
    0x5A8, 0x5A9, 0x5AB, 0x5AC, 0x5AF, 0x5C4, 0x610, 0x611, 0x612, 0x613, 0x614, 0x615,
    0x616, 0x617, 0x657, 0x658, 0x659, 0x65A, 0x65B, 0x65D, 0x65E, 0x6D6, 0x6D7, 0x6D8,
    0x6D9, 0x6DA, 0x6DB, 0x6DC, 0x6DF, 0x6E0, 0x6E1, 0x6E2, 0x6E4, 0x6E7, 0x6E8, 0x6EB,
    0x6EC, 0x730, 0x732, 0x733, 0x735, 0x736, 0x73A, 0x73D, 0x73F, 0x740, 0x741, 0x743,
    0x745, 0x747, 0x749, 0x74A, 0x7EB, 0x7EC, 0x7ED, 0x7EE, 0x7EF, 0x7F0, 0x7F1, 0x7F3,
    0x816, 0x817, 0x818, 0x819, 0x81B, 0x81C, 0x81D, 0x81E, 0x81F, 0x820, 0x821, 0x822,
    0x823, 0x825, 0x826, 0x827, 0x829, 0x82A, 0x82B, 0x82C, 0x82D, 0x951, 0x953, 0x954,
    0xF82, 0xF83, 0xF86, 0xF87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71,
    0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7,
    0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE,
    0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1,
    0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED,
    0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1,
    0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187,
    0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

/// The row or column a diacritic stands for.
pub fn index_of(c: char) -> Option<usize> {
    DIACRITICS.binary_search(&(c as u32)).ok()
}

/// The diacritic for a row or column number, if the list reaches that far.
#[cfg(test)]
pub fn diacritic(index: usize) -> Option<char> {
    DIACRITICS.get(index).and_then(|&c| char::from_u32(c))
}

/// The image id a placeholder cell names: the color of the text, with the top 8 bits from a
/// third diacritic when there is one.
fn image_key(fg: Color, third: Option<usize>) -> Option<u32> {
    let low = match fg {
        Color::Rgb(r, g, b) => u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
        Color::Indexed(n) => u32::from(n),
        Color::Default => return None,
    };
    Some((third.map_or(0, |i| (i & 0xFF) as u32) << 24) | low)
}

impl Grid {
    /// Show the piece of a picture that the placeholder at (`x`, `y`) stands for, now that the
    /// cell is written or has got another diacritic. A placeholder that names no picture stays a
    /// plain character.
    pub(super) fn resolve_placeholder(&mut self, y: usize, x: usize) {
        let tile = self.placeholder_tile(y, x).unwrap_or(0);
        self.screen[y].cells[x].tile = tile;
    }

    fn placeholder_tile(&mut self, y: usize, x: usize) -> Option<u32> {
        let cell = self.screen[y].cells[x];
        let marks = marks::chars(cell.mark);
        let at = |i: usize| marks.get(i).and_then(|&c| index_of(c));
        let key = image_key(cell.style.fg, at(2))?;
        let &(image, cols, rows) = self.virtual_images.get(&key)?;
        self.images.image(image)?;
        let left = x.checked_sub(1).map(|l| self.screen[y].cells[l]).filter(|l| l.ch == PLACEHOLDER);
        let left = left.and_then(|l| self.images.lookup(l.tile)).filter(|t| t.image == image);
        let (row, col) = match (at(0), at(1)) {
            (Some(r), Some(c)) => (r, c),
            (Some(r), None) => (r, left.filter(|t| usize::from(t.ty) == r).map_or(0, |t| usize::from(t.tx) + 1)),
            _ => left.map_or((0, 0), |t| (usize::from(t.ty), usize::from(t.tx) + 1)),
        };
        if row >= rows || col >= cols {
            return None;
        }
        Some(self.images.tile(image, col as u16, row as u16))
    }

    /// Remember that a picture is shown by placeholders under the id `key`.
    pub(super) fn add_virtual(&mut self, key: u32, image: ImageId, cols: usize, rows: usize) {
        self.images.pin(image);
        if let Some((old, _, _)) = self.virtual_images.insert(key, (image, cols, rows)) {
            self.images.unpin(old);
        }
    }

    /// Forget the placeholder picture `key`; its cells go on showing it until they are erased.
    pub(super) fn drop_virtual(&mut self, key: u32) -> Option<ImageId> {
        let (image, _, _) = self.virtual_images.remove(&key)?;
        self.images.unpin(image);
        Some(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Bitmap;
    use crate::term::cell::Cell;
    use crate::term::Terminal;

    #[test]
    fn the_list_is_the_protocols_and_sorted() {
        assert_eq!(DIACRITICS.len(), 297);
        assert!(DIACRITICS.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(index_of('\u{305}'), Some(0));
        assert_eq!(index_of('\u{30D}'), Some(1));
        assert_eq!(index_of('\u{30E}'), Some(2));
        assert_eq!(index_of('\u{310}'), Some(3));
        assert_eq!(index_of('\u{312}'), Some(4));
        assert_eq!(index_of('\u{33D}'), Some(5));
        assert_eq!(index_of('\u{1D244}'), Some(296));
        assert_eq!(index_of('\u{300}'), None, "a grave accent composes with letters: not in the list");
        assert_eq!(diacritic(2), Some('\u{30E}'));
    }

    fn term_with_picture(key: u32, cols: usize, rows: usize) -> Terminal {
        let mut t = Terminal::new(20, 8, 100);
        t.grid.cell_px = (10, 20);
        let image = t.grid.images.add(Bitmap::new(cols * 10, rows * 20), cols, rows);
        t.grid.add_virtual(key, image, cols, rows);
        t
    }

    fn piece(t: &Terminal, x: usize, y: usize) -> Option<(u16, u16)> {
        let cell = t.grid.screen[y].cells[x];
        t.grid.images.lookup(cell.tile).map(|tile| (tile.tx, tile.ty))
    }

    #[test]
    fn placeholders_with_row_and_column_show_those_pieces() {
        let mut t = term_with_picture(0x0000FF, 4, 3);
        t.feed("\x1b[38;2;0;0;255m\u{10EEEE}\u{30E}\u{30D}\u{10EEEE}\u{30E}\u{310}".as_bytes());
        assert_eq!(piece(&t, 0, 0), Some((1, 2)), "row 2, column 1");
        assert_eq!(piece(&t, 1, 0), Some((3, 2)), "row 2, column 3");
    }

    #[test]
    fn placeholders_without_diacritics_continue_to_the_right() {
        let mut t = term_with_picture(7, 4, 2);
        t.feed("\x1b[38;5;7m\u{10EEEE}\u{305}\u{305}\u{10EEEE}\u{10EEEE}".as_bytes());
        assert_eq!((piece(&t, 0, 0), piece(&t, 1, 0), piece(&t, 2, 0)), (Some((0, 0)), Some((1, 0)), Some((2, 0))));
        let mut t = term_with_picture(7, 4, 2);
        t.feed("\x1b[38;5;7m\u{10EEEE}\u{30D}\u{10EEEE}\u{30D}".as_bytes());
        assert_eq!((piece(&t, 0, 0), piece(&t, 1, 0)), (Some((0, 1)), Some((1, 1))), "only the row given: the column follows");
    }

    #[test]
    fn a_third_diacritic_adds_the_top_eight_bits_of_the_id() {
        let mut t = term_with_picture(0x0100_0005, 2, 2);
        t.feed("\x1b[38;5;5m\u{10EEEE}\u{305}\u{305}\u{30D}".as_bytes());
        assert_eq!(piece(&t, 0, 0), Some((0, 0)));
        let mut t = term_with_picture(5, 2, 2);
        t.feed("\x1b[38;5;5m\u{10EEEE}\u{305}\u{305}\u{30D}".as_bytes());
        assert_eq!(piece(&t, 0, 0), None, "id 0x01000005 is another picture");
    }

    #[test]
    fn an_unknown_id_or_a_piece_outside_the_picture_stays_a_plain_character() {
        let mut t = term_with_picture(1, 2, 2);
        t.feed("\x1b[38;5;9m\u{10EEEE}".as_bytes());
        assert_eq!(piece(&t, 0, 0), None);
        t.feed("\r\n\x1b[38;5;1m\u{10EEEE}\u{36F}\u{305}".as_bytes());
        assert_eq!(piece(&t, 0, 1), None, "row 29 of a 2 row picture");
        t.feed("\r\n\u{10EEEE}".as_bytes());
        t.feed("\x1b[0m\r\n\u{10EEEE}".as_bytes());
        assert_eq!(piece(&t, 0, 3), None, "no color, no id");
    }

    #[test]
    fn the_picture_survives_a_collection_while_it_is_pinned() {
        let mut t = term_with_picture(3, 2, 1);
        t.grid.collect_images();
        t.feed("\x1b[38;5;3m\u{10EEEE}".as_bytes());
        assert_eq!(piece(&t, 0, 0), Some((0, 0)), "no cell used it yet, but the program may still print it");
    }

    #[test]
    fn text_over_a_placeholder_picture_is_the_picture_text() {
        let mut t = term_with_picture(3, 3, 1);
        t.feed("a\x1b[38;5;3m\u{10EEEE}\u{305}\u{305}\u{10EEEE}\u{10EEEE}b".as_bytes());
        let cells: Vec<Cell> = t.grid.screen[0].cells[..5].to_vec();
        assert_eq!(Cell::text_for_copy(&cells), "a[image]b");
    }
}
