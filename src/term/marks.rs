//! Zero-width characters (a combining accent, a variation selector, a joiner) belong to the cell
//! printed before them. A cell stays `Copy` and small by holding only an index into a table of
//! the marks seen so far; every copy of the cell (scrollback, reflow, the compositor) carries its
//! marks with it, and cells with the same marks compare equal.
//!
//! At most `PER_CELL` marks are kept for a cell and `TABLE_LIMIT` different mark sequences for the
//! whole process; what does not fit is dropped, as it was before marks were kept at all. The
//! table lives per thread: the emulator and the compositor run on the server thread.

use std::cell::RefCell;
use std::collections::HashMap;

/// Marks kept for one cell.
pub const PER_CELL: usize = 3;
/// Marks kept for one cell that stands for a whole cluster (`emoji-width cluster`): a family of
/// four people is the first person and six more characters.
pub const PER_CLUSTER: usize = 11;
/// Different mark sequences remembered. A cell holding index 0 has none.
pub const TABLE_LIMIT: usize = 4096;

type Seq = [char; PER_CLUSTER];

#[derive(Default)]
struct Table {
    seqs: Vec<Seq>,
    index: HashMap<Seq, u16>,
}

thread_local! {
    static TABLE: RefCell<Table> = RefCell::new(Table::default());
}

fn len(seq: &Seq) -> usize {
    seq.iter().take_while(|c| **c != '\0').count()
}

/// The index for the marks of `mark` followed by `c`. `mark` itself when the cell is full or the
/// table is, so the new character is dropped.
#[cfg(test)]
pub fn add(mark: u16, c: char) -> u16 {
    add_up_to(mark, c, PER_CELL)
}

/// The marks of index `mark`, in order.
pub fn chars(mark: u16) -> Vec<char> {
    if mark == 0 {
        return Vec::new();
    }
    TABLE.with(|t| t.borrow().seqs.get(usize::from(mark) - 1).map(|seq| seq[..len(seq)].to_vec()).unwrap_or_default())
}

/// The last mark of index `mark`, if there is one.
pub fn last(mark: u16) -> Option<char> {
    if mark == 0 {
        return None;
    }
    TABLE.with(|t| t.borrow().seqs.get(usize::from(mark) - 1).and_then(|seq| seq[..len(seq)].last().copied()))
}

/// `add` with room for `cap` marks (at most `PER_CLUSTER`).
pub fn add_up_to(mark: u16, c: char, cap: usize) -> u16 {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        let mut seq: Seq = if mark == 0 { ['\0'; PER_CLUSTER] } else { t.seqs[usize::from(mark) - 1] };
        let n = len(&seq);
        if n >= cap.min(PER_CLUSTER) {
            return mark;
        }
        seq[n] = c;
        if let Some(i) = t.index.get(&seq) {
            return *i;
        }
        if t.seqs.len() >= TABLE_LIMIT {
            return mark;
        }
        t.seqs.push(seq);
        let i = t.seqs.len() as u16;
        t.index.insert(seq, i);
        i
    })
}

/// Append the marks of index `mark` to `out`.
pub fn push_to(out: &mut String, mark: u16) {
    if mark == 0 {
        return;
    }
    TABLE.with(|t| {
        if let Some(seq) = t.borrow().seqs.get(usize::from(mark) - 1) {
            out.extend(seq.iter().take_while(|c| **c != '\0'));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(mark: u16) -> String {
        let mut s = String::new();
        push_to(&mut s, mark);
        s
    }

    #[test]
    fn marks_are_collected_in_order_and_equal_sequences_share_an_index() {
        let a = add(add(0, '\u{301}'), '\u{302}');
        assert_eq!(text(a), "\u{301}\u{302}");
        assert_eq!(add(add(0, '\u{301}'), '\u{302}'), a);
        assert_ne!(add(add(0, '\u{302}'), '\u{301}'), a);
        assert_eq!(text(0), "");
    }

    #[test]
    fn a_cell_stays_small_with_its_mark_index() {
        assert!(std::mem::size_of::<crate::term::cell::Cell>() <= 24, "{}", std::mem::size_of::<crate::term::cell::Cell>());
    }

    #[test]
    fn a_cell_keeps_at_most_three_marks() {
        let mut m = 0;
        for c in ['\u{301}', '\u{302}', '\u{303}', '\u{304}', '\u{305}'] {
            m = add(m, c);
        }
        assert_eq!(text(m), "\u{301}\u{302}\u{303}");
    }

    #[test]
    fn a_full_table_drops_new_sequences_but_keeps_the_old_ones() {
        let first = add(0, '\u{300}');
        let mut last = first;
        for i in 0..(TABLE_LIMIT as u32 + 10) {
            if let Some(c) = char::from_u32(0x4e00 + i) {
                last = add(0, c);
            }
        }
        assert_eq!(add(0, '\u{300}'), first, "known sequences still resolve");
        assert!(last <= TABLE_LIMIT as u16);
        let before = text(first);
        assert_eq!(before, "\u{300}");
    }
}
