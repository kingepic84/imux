//! tmux's layout strings (`#{window_layout}`, `select-layout "<string>"`): the way plugins save and
//! put back the sizes of the panes of a window. The string is `CCCC,` (a checksum) and a node:
//! `WxH,X,Y,ID` for a pane, `WxH,X,Y{a,b,...}` for panes side by side and `WxH,X,Y[a,b,...]` for
//! panes on top of each other. imux holds splits as a binary tree of ratios, so a node with
//! several children is read as a chain of splits and written back by joining a chain.

use super::{Layout, Node, PaneId, Rect, Split, children};

/// Longest layout string read, deepest nesting and most panes: a string is data from a file.
const MAX_TEXT: usize = 8192;
const MAX_DEPTH: usize = 16;
const MAX_PANES: usize = 128;

/// tmux's checksum of a layout body: a rotate-right-and-add over the bytes.
pub fn checksum(body: &str) -> u16 {
    let mut sum: u16 = 0;
    for b in body.bytes() {
        sum = (sum >> 1).wrapping_add((sum & 1) << 15);
        sum = sum.wrapping_add(u16::from(b));
    }
    sum
}

/// A node of tmux's tree: sizes in cells, children in order.
#[derive(Debug, PartialEq)]
enum Tree {
    Pane { w: u16, h: u16, x: u16, y: u16, id: Option<u32> },
    Group { kind: Split, w: u16, h: u16, x: u16, y: u16, kids: Vec<Tree> },
}

impl Tree {
    fn size(&self) -> (u16, u16) {
        match self {
            Tree::Pane { w, h, .. } | Tree::Group { w, h, .. } => (*w, *h),
        }
    }
}

fn tree_of(node: &Node, r: Rect, origin: (u16, u16)) -> Tree {
    match node {
        Node::Leaf(id) => Tree::Pane { w: r.w, h: r.h, x: r.x - origin.0, y: r.y - origin.1, id: Some(*id) },
        Node::Branch { kind, ratio, a, b } => {
            let (ra, rb, _) = children(*kind, *ratio, r);
            let mut kids = Vec::new();
            for (n, rect) in [(a, ra), (b, rb)] {
                match tree_of(n, rect, origin) {
                    Tree::Group { kind: k, kids: inner, .. } if k == *kind => kids.extend(inner),
                    other => kids.push(other),
                }
            }
            Tree::Group { kind: *kind, w: r.w, h: r.h, x: r.x - origin.0, y: r.y - origin.1, kids }
        }
    }
}

fn write(tree: &Tree, out: &mut String) {
    use std::fmt::Write;
    match tree {
        Tree::Pane { w, h, x, y, id } => {
            let _ = write!(out, "{w}x{h},{x},{y}");
            if let Some(id) = id {
                let _ = write!(out, ",{id}");
            }
        }
        Tree::Group { kind, w, h, x, y, kids } => {
            let _ = write!(out, "{w}x{h},{x},{y}");
            let (open, close) = if *kind == Split::SideBySide { ('{', '}') } else { ('[', ']') };
            out.push(open);
            for (i, k) in kids.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(k, out);
            }
            out.push(close);
        }
    }
}

struct Reader<'a> {
    rest: &'a [u8],
    panes: usize,
}

impl Reader<'_> {
    fn number(&mut self) -> Result<u32, String> {
        let digits = self.rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 || digits > 5 {
            return Err("invalid layout".to_string());
        }
        let n = std::str::from_utf8(&self.rest[..digits]).map_err(|_| "invalid layout")?.parse::<u32>().map_err(|_| "invalid layout")?;
        self.rest = &self.rest[digits..];
        Ok(n)
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.rest.first() == Some(&byte) {
            self.rest = &self.rest[1..];
            true
        } else {
            false
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.eat(byte) { Ok(()) } else { Err("invalid layout".to_string()) }
    }

    fn size(&mut self) -> Result<u16, String> {
        u16::try_from(self.number()?).map_err(|_| "invalid layout".to_string())
    }

    fn node(&mut self, depth: usize) -> Result<Tree, String> {
        if depth > MAX_DEPTH {
            return Err("layout is nested too deep".to_string());
        }
        let w = self.size()?;
        self.expect(b'x')?;
        let h = self.size()?;
        self.expect(b',')?;
        let x = self.size()?;
        self.expect(b',')?;
        let y = self.size()?;
        let kind = match self.rest.first() {
            Some(b'{') => Some((Split::SideBySide, b'}')),
            Some(b'[') => Some((Split::Stacked, b']')),
            _ => None,
        };
        let Some((kind, close)) = kind else {
            self.panes += 1;
            if self.panes > MAX_PANES {
                return Err("layout has too many panes".to_string());
            }
            let id = if self.eat(b',') { Some(self.number()?) } else { None };
            return Ok(Tree::Pane { w, h, x, y, id });
        };
        self.rest = &self.rest[1..];
        let mut kids = vec![self.node(depth + 1)?];
        while self.eat(b',') {
            kids.push(self.node(depth + 1)?);
        }
        self.expect(close)?;
        Ok(Tree::Group { kind, w, h, x, y, kids })
    }
}

/// The tree of a layout string; its checksum has to be right.
fn read(text: &str) -> Result<Tree, String> {
    if text.len() > MAX_TEXT || !text.is_ascii() {
        return Err("invalid layout".to_string());
    }
    let (sum, body) = text.split_once(',').ok_or("invalid layout")?;
    if sum.len() != 4 || u16::from_str_radix(sum, 16).map_err(|_| "invalid layout")? != checksum(body) {
        return Err("invalid layout".to_string());
    }
    let mut reader = Reader { rest: body.as_bytes(), panes: 0 };
    let tree = reader.node(0)?;
    if !reader.rest.is_empty() {
        return Err("invalid layout".to_string());
    }
    check(&tree)?;
    Ok(tree)
}

/// Children must fill their group: along the split their sizes and the one-cell dividers add up to
/// the group, across it each is as big as the group.
fn check(tree: &Tree) -> Result<(), String> {
    let Tree::Group { kind, w, h, kids, .. } = tree else { return Ok(()) };
    let (along, across) = if *kind == Split::SideBySide { (*w, *h) } else { (*h, *w) };
    let mut used = kids.len() as u32 - 1;
    for kid in kids {
        let (kw, kh) = kid.size();
        let (k_along, k_across) = if *kind == Split::SideBySide { (kw, kh) } else { (kh, kw) };
        if k_across != across || k_along == 0 {
            return Err("invalid layout".to_string());
        }
        used += u32::from(k_along);
        check(kid)?;
    }
    if used != u32::from(along) {
        return Err("invalid layout".to_string());
    }
    Ok(())
}

/// A chain of splits for the children of one group; `leaf` gives each pane its id.
fn node_of(tree: &Tree, leaf: &mut impl FnMut() -> PaneId) -> Node {
    match tree {
        Tree::Pane { .. } => Node::Leaf(leaf()),
        Tree::Group { kind, kids, .. } => {
            let sizes: Vec<u16> = kids.iter().map(|k| if *kind == Split::SideBySide { k.size().0 } else { k.size().1 }).collect();
            chain_of(*kind, kids, &sizes, leaf)
        }
    }
}

fn chain_of(kind: Split, kids: &[Tree], sizes: &[u16], leaf: &mut impl FnMut() -> PaneId) -> Node {
    let first = node_of(&kids[0], leaf);
    if kids.len() == 1 {
        return first;
    }
    let rest_total: u32 = sizes[1..].iter().map(|s| u32::from(*s)).sum::<u32>() + (sizes.len() as u32 - 2);
    let avail = u32::from(sizes[0]) + rest_total;
    let ratio = if avail == 0 { 0.5 } else { f32::from(sizes[0]) / avail as f32 };
    let b = chain_of(kind, &kids[1..], &sizes[1..], leaf);
    Node::Branch { kind, ratio, a: Box::new(first), b: Box::new(b) }
}

fn pane_count(tree: &Tree) -> usize {
    match tree {
        Tree::Pane { .. } => 1,
        Tree::Group { kids, .. } => kids.iter().map(pane_count).sum(),
    }
}

impl Layout {
    /// The layout as tmux writes it, for a window of `area` (`#{window_layout}`).
    pub fn to_tmux(&self, area: Rect) -> String {
        let tree = tree_of(&self.root, area, (area.x, area.y));
        let mut body = String::new();
        write(&tree, &mut body);
        format!("{:04x},{body}", checksum(&body))
    }

    /// A layout from tmux's text for the panes `panes` (in their order): the splits and sizes are the
    /// text's, the panes are those given, one for each pane of the text. An error names what is
    /// wrong, in tmux's words where it has them.
    pub fn from_tmux(text: &str, panes: &[PaneId]) -> Result<Layout, String> {
        let tree = read(text)?;
        let wanted = pane_count(&tree);
        if wanted != panes.len() {
            return Err(format!("have {} panes but need {wanted}", panes.len()));
        }
        let mut ids = panes.iter().copied();
        let mut next = || ids.next().unwrap_or_default();
        Ok(Layout { root: node_of(&tree, &mut next), zoom: None })
    }
}

/// Whether `text` is the shape of a layout string, for a parser that has to tell it from a name.
pub fn looks_like_layout(text: &str) -> bool {
    text.len() > 5 && text.as_bytes().get(4) == Some(&b',') && text[..4].bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0, y: 0, w: 80, h: 24 };

    fn split(l: &mut Layout, at: PaneId, kind: Split, new: PaneId) {
        l.split(at, kind, new, AREA).unwrap();
    }

    #[test]
    fn the_checksum_is_the_one_tmux_computes() {
        assert_eq!(checksum(""), 0);
        assert_eq!(checksum("a"), 97);
        assert_eq!(checksum("ab"), (97 >> 1) + ((97 & 1) << 15) + 98);
        let body = "80x24,0,0,0";
        let text = format!("{:04x},{body}", checksum(body));
        assert!(read(&text).is_ok());
    }

    #[test]
    fn a_single_pane_is_one_leaf_of_the_window_size() {
        let l = Layout::new(7);
        let text = l.to_tmux(AREA);
        assert!(text.ends_with(",80x24,0,0,7"), "{text}");
    }

    #[test]
    fn side_by_side_panes_are_braces_and_stacked_ones_are_brackets() {
        let mut l = Layout::new(0);
        split(&mut l, 0, Split::SideBySide, 1);
        let text = l.to_tmux(AREA);
        assert!(text.ends_with(",80x24,0,0{40x24,0,0,0,39x24,41,0,1}"), "{text}");
        let mut l = Layout::new(0);
        split(&mut l, 0, Split::Stacked, 1);
        assert!(l.to_tmux(AREA).ends_with(",80x24,0,0[80x12,0,0,0,80x11,0,13,1]"), "{}", l.to_tmux(AREA));
    }

    #[test]
    fn three_panes_in_a_row_are_one_group_not_nested() {
        let mut l = Layout::new(0);
        split(&mut l, 0, Split::SideBySide, 1);
        split(&mut l, 1, Split::SideBySide, 2);
        let text = l.to_tmux(AREA);
        assert_eq!(text.matches('{').count(), 1, "{text}");
        assert!(text.ends_with(",80x24,0,0{40x24,0,0,0,19x24,41,0,1,19x24,61,0,2}"), "{text}");
    }

    #[test]
    fn a_layout_written_and_read_back_gives_the_same_places() {
        let mut l = Layout::new(0);
        split(&mut l, 0, Split::SideBySide, 1);
        split(&mut l, 1, Split::Stacked, 2);
        split(&mut l, 0, Split::Stacked, 3);
        l.resize(3, super::super::Direction::Up, 4, AREA);
        let text = l.to_tmux(AREA);
        let again = Layout::from_tmux(&text, &l.panes()).unwrap();
        assert_eq!(again.rects(AREA), l.rects(AREA), "{text}");
        assert_eq!(again.to_tmux(AREA), text);
    }

    #[test]
    fn a_layout_is_scaled_to_the_window_it_is_put_on() {
        let mut l = Layout::new(0);
        split(&mut l, 0, Split::SideBySide, 1);
        let text = l.to_tmux(AREA);
        let wide = Rect { x: 0, y: 0, w: 161, h: 50 };
        let on = Layout::from_tmux(&text, &[5, 6]).unwrap();
        let rects = on.rects(wide);
        assert_eq!(rects.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [5, 6]);
        assert_eq!(rects[0].1.w + rects[1].1.w, 160, "the divider takes one column");
        assert!(rects[0].1.w.abs_diff(80) <= 1, "each half is about half of the new width");
        assert!(rects.iter().all(|(_, r)| r.h == 50));
    }

    #[test]
    fn mistakes_are_refused_by_name() {
        let ok = format!("{:04x},80x24,0,0{{40x24,0,0,0,39x24,41,0,1}}", checksum("80x24,0,0{40x24,0,0,0,39x24,41,0,1}"));
        assert!(Layout::from_tmux(&ok, &[0, 1]).is_ok());
        assert_eq!(Layout::from_tmux(&ok, &[0]).unwrap_err(), "have 1 panes but need 2");
        let bad_sum = format!("0000{}", &ok[4..]);
        assert_eq!(Layout::from_tmux(&bad_sum, &[0, 1]).unwrap_err(), "invalid layout");
        for body in ["80x24,0,0{40x24,0,0,0,40x24,41,0,1}", "80x24,0,0{40x24,0,0,0}", "80x24,0,0{40x24,0,0,0,39x23,41,0,1}", "80x24", "80x24,0,0{", "x", "80x24,0,0,0,"] {
            let text = format!("{:04x},{body}", checksum(body));
            assert!(read(&text).is_err(), "{body}");
        }
        assert!(read("zzzz,80x24,0,0,0").is_err());
        assert!(read(&"1".repeat(MAX_TEXT + 1)).is_err());
        let deep = format!("{}80x24,0,0,0{}", "80x24,0,0[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert!(read(&format!("{:04x},{deep}", checksum(&deep))).is_err());
    }

    #[test]
    fn the_shape_of_a_layout_string_is_told_from_a_name() {
        assert!(looks_like_layout("bb62,159x48,0,0{79x48,0,0,0,79x48,80,0,1}"));
        assert!(!looks_like_layout("tiled"));
        assert!(!looks_like_layout("even-horizontal"));
        assert!(!looks_like_layout("main-vertical"));
    }
}
