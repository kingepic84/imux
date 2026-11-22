//! Split tree of one window: which pane sits where, with one-cell separators between siblings.

use crate::target::{Horiz, Vert};

mod text;
pub use text::looks_like_layout;

pub type PaneId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// How a node divides its area between its two children.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    /// Children left and right, divider is a vertical line (tmux `split-window -h`).
    SideBySide,
    /// Children top and bottom, divider is a horizontal line (tmux `split-window -v`).
    Stacked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// A one-cell-wide divider. `vertical` lines run down `len` rows from `(x, y)`, horizontal ones
/// run right `len` columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Separator {
    pub x: u16,
    pub y: u16,
    pub len: u16,
    pub vertical: bool,
}

#[derive(Debug)]
enum Node {
    Leaf(PaneId),
    Branch { kind: Split, ratio: f32, a: Box<Node>, b: Box<Node> },
}

#[derive(Debug)]
pub struct Layout {
    root: Node,
    /// The pane shown alone at the full size of the window (`resize-pane -Z`); the others keep
    /// their place in the tree and come back when it is unzoomed.
    zoom: Option<PaneId>,
}

/// Named arrangements of all the panes of a window (`select-layout`, `next-layout`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    EvenHorizontal,
    EvenVertical,
    MainHorizontal,
    MainHorizontalMirrored,
    MainVertical,
    MainVerticalMirrored,
    Tiled,
}

impl Preset {
    /// In the order `next-layout` goes through them.
    pub const ALL: [Preset; 7] = [
        Preset::EvenHorizontal,
        Preset::EvenVertical,
        Preset::MainHorizontal,
        Preset::MainHorizontalMirrored,
        Preset::MainVertical,
        Preset::MainVerticalMirrored,
        Preset::Tiled,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Preset::EvenHorizontal => "even-horizontal",
            Preset::EvenVertical => "even-vertical",
            Preset::MainHorizontal => "main-horizontal",
            Preset::MainHorizontalMirrored => "main-horizontal-mirrored",
            Preset::MainVertical => "main-vertical",
            Preset::MainVerticalMirrored => "main-vertical-mirrored",
            Preset::Tiled => "tiled",
        }
    }

    pub fn parse(name: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.name() == name)
    }

    /// Position in `ALL`.
    pub fn index(self) -> usize {
        Preset::ALL.iter().position(|&p| p == self).unwrap_or(0)
    }
}

/// Width of the main pane of `main-vertical` and height of the one of `main-horizontal`, at most
/// (two thirds of the window when that is less).
const MAIN_COLS: u16 = 80;
const MAIN_ROWS: u16 = 24;

/// A layout with something other than a pane id in each leaf, to take a layout apart and build
/// one again (saving a session, restoring it). Leaves are in the order of `Layout::panes`.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape<T> {
    Leaf(T),
    Branch { kind: Split, ratio: f32, a: Box<Shape<T>>, b: Box<Shape<T>> },
}

impl<T> Shape<T> {
    /// The same splits with every leaf turned into something else, in leaf order.
    pub fn map<U>(self, f: &mut impl FnMut(T) -> U) -> Shape<U> {
        match self {
            Shape::Leaf(t) => Shape::Leaf(f(t)),
            Shape::Branch { kind, ratio, a, b } => {
                let a = Box::new(a.map(f));
                let b = Box::new(b.map(f));
                Shape::Branch { kind, ratio, a, b }
            }
        }
    }

    #[cfg(test)]
    pub fn leaf_count(&self) -> usize {
        match self {
            Shape::Leaf(_) => 1,
            Shape::Branch { a, b, .. } => a.leaf_count() + b.leaf_count(),
        }
    }
}

fn shape_of<T>(node: &Node, leaf: &mut impl FnMut(PaneId) -> T) -> Shape<T> {
    match node {
        Node::Leaf(id) => Shape::Leaf(leaf(*id)),
        Node::Branch { kind, ratio, a, b } => {
            let a = Box::new(shape_of(a, leaf));
            let b = Box::new(shape_of(b, leaf));
            Shape::Branch { kind: *kind, ratio: *ratio, a, b }
        }
    }
}

fn node_of<T>(shape: Shape<T>, leaf: &mut impl FnMut(T) -> PaneId) -> Node {
    match shape {
        Shape::Leaf(t) => Node::Leaf(leaf(t)),
        Shape::Branch { kind, ratio, a, b } => {
            let ratio = if ratio.is_finite() { ratio.clamp(0.0, 1.0) } else { 0.5 };
            let a = Box::new(node_of(*a, leaf));
            let b = Box::new(node_of(*b, leaf));
            Node::Branch { kind, ratio, a, b }
        }
    }
}

/// Cells given to the first and second child of a branch spanning `total` cells (one is the divider).
fn divide(total: u16, ratio: f32) -> (u16, u16) {
    let avail = total.saturating_sub(1);
    if avail < 2 {
        return (avail, 0);
    }
    let a = ((ratio * f32::from(avail)).round() as u16).clamp(1, avail - 1);
    (a, avail - a)
}

fn children(kind: Split, ratio: f32, r: Rect) -> (Rect, Rect, Separator) {
    match kind {
        Split::SideBySide => {
            let (wa, wb) = divide(r.w, ratio);
            (
                Rect { w: wa, ..r },
                Rect { x: r.x + wa + 1, w: wb, ..r },
                Separator { x: r.x + wa, y: r.y, len: r.h, vertical: true },
            )
        }
        Split::Stacked => {
            let (ha, hb) = divide(r.h, ratio);
            (
                Rect { h: ha, ..r },
                Rect { y: r.y + ha + 1, h: hb, ..r },
                Separator { x: r.x, y: r.y + ha, len: r.w, vertical: false },
            )
        }
    }
}

fn collect(node: &Node, r: Rect, rects: &mut Vec<(PaneId, Rect)>, seps: &mut Vec<Separator>) {
    match node {
        Node::Leaf(id) => rects.push((*id, r)),
        Node::Branch { kind, ratio, a, b } => {
            let (ra, rb, sep) = children(*kind, *ratio, r);
            seps.push(sep);
            collect(a, ra, rects, seps);
            collect(b, rb, rects, seps);
        }
    }
}

fn contains(node: &Node, target: PaneId) -> bool {
    match node {
        Node::Leaf(id) => *id == target,
        Node::Branch { a, b, .. } => contains(a, target) || contains(b, target),
    }
}

fn leaves(node: &Node, out: &mut Vec<PaneId>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Branch { a, b, .. } => {
            leaves(a, out);
            leaves(b, out);
        }
    }
}

fn replace_leaf(node: &mut Node, target: PaneId, kind: Split, new: PaneId) -> bool {
    match node {
        Node::Leaf(id) if *id == target => {
            *node = Node::Branch {
                kind,
                ratio: 0.5,
                a: Box::new(Node::Leaf(target)),
                b: Box::new(Node::Leaf(new)),
            };
            true
        }
        Node::Leaf(_) => false,
        Node::Branch { a, b, .. } => replace_leaf(a, target, kind, new) || replace_leaf(b, target, kind, new),
    }
}

fn remove_leaf(node: Node, target: PaneId) -> Option<Node> {
    match node {
        Node::Leaf(id) => (id != target).then_some(Node::Leaf(id)),
        Node::Branch { kind, ratio, a, b } => match (remove_leaf(*a, target), remove_leaf(*b, target)) {
            (Some(a), Some(b)) => Some(Node::Branch { kind, ratio, a: Box::new(a), b: Box::new(b) }),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        },
    }
}

/// Moves the divider of the closest ancestor of `target` that splits along `axis`.
/// `None`: target not under this node. `Some(false)`: no such ancestor here.
fn move_divider(node: &mut Node, target: PaneId, axis: Split, delta: i32, r: Rect) -> Option<bool> {
    let Node::Branch { kind, ratio, a, b } = node else {
        return matches!(node, Node::Leaf(id) if *id == target).then_some(false);
    };
    let (ra, rb, _) = children(*kind, *ratio, r);
    let inner = if contains(a, target) {
        move_divider(a, target, axis, delta, ra)
    } else {
        move_divider(b, target, axis, delta, rb)
    }?;
    if inner {
        return Some(true);
    }
    if *kind != axis {
        return Some(false);
    }
    let total = if axis == Split::SideBySide { r.w } else { r.h };
    let avail = total.saturating_sub(1);
    if avail < 2 {
        return Some(true);
    }
    let (cur, _) = divide(total, *ratio);
    let next = (i32::from(cur) + delta).clamp(1, i32::from(avail) - 1);
    *ratio = next as f32 / f32::from(avail);
    Some(true)
}

/// Puts the divider `sep` at column/row `pos`, keeping both sides at least one cell. Returns the
/// divider as it is afterwards, or `None` when no divider of this node is `sep`.
fn set_divider(node: &mut Node, r: Rect, sep: Separator, pos: u16) -> Option<Separator> {
    let Node::Branch { kind, ratio, a, b } = node else { return None };
    let (ra, rb, current) = children(*kind, *ratio, r);
    if current == sep {
        let (total, start) = if *kind == Split::SideBySide { (r.w, r.x) } else { (r.h, r.y) };
        let avail = total.saturating_sub(1);
        if avail < 2 {
            return Some(current);
        }
        let first = pos.saturating_sub(start).clamp(1, avail - 1);
        *ratio = f32::from(first) / f32::from(avail);
        return Some(children(*kind, *ratio, r).2);
    }
    set_divider(a, ra, sep, pos).or_else(|| set_divider(b, rb, sep, pos))
}

impl Layout {
    /// Drag the divider `sep` to the pointer position (`x` for vertical dividers, `y` for horizontal).
    /// Returns where the divider ended up so the drag can follow it.
    pub fn drag_divider(&mut self, sep: Separator, pos: u16, area: Rect) -> Option<Separator> {
        set_divider(&mut self.root, area, sep, pos)
    }

    pub fn new(first: PaneId) -> Layout {
        Layout { root: Node::Leaf(first), zoom: None }
    }

    /// The zoomed pane, if one is.
    pub fn zoomed(&self) -> Option<PaneId> {
        self.zoom.filter(|&z| contains(&self.root, z))
    }

    pub fn unzoom(&mut self) {
        self.zoom = None;
    }

    /// Zoom `target`, or unzoom when it is zoomed already. A window with one pane has nothing to
    /// zoom; the answer is whether a pane is zoomed afterwards.
    pub fn toggle_zoom(&mut self, target: PaneId) -> bool {
        if self.zoomed().is_some() {
            self.zoom = None;
        } else if contains(&self.root, target) && self.panes().len() > 1 {
            self.zoom = Some(target);
        }
        self.zoom.is_some()
    }

    /// Exchange the places of two panes.
    pub fn swap(&mut self, a: PaneId, b: PaneId) -> bool {
        if a == b || !contains(&self.root, a) || !contains(&self.root, b) {
            return false;
        }
        swap_ids(&mut self.root, a, b);
        true
    }

    /// Put pane `new` where pane `old` is (the two live in different windows): false when `old`
    /// is not here or `new` already is.
    pub fn replace(&mut self, old: PaneId, new: PaneId) -> bool {
        if !contains(&self.root, old) || contains(&self.root, new) {
            return false;
        }
        self.zoom = None;
        swap_ids(&mut self.root, old, new);
        true
    }

    /// Move every pane to the next place (`down`) or the previous one, the end wrapping round.
    pub fn rotate(&mut self, down: bool) {
        let mut ids = self.panes();
        if ids.len() < 2 {
            return;
        }
        if down {
            ids.rotate_right(1);
        } else {
            ids.rotate_left(1);
        }
        assign(&mut self.root, &mut ids.into_iter());
    }

    /// Arrange all panes as `preset` says, in their current order; the first is the main pane.
    pub fn apply(&mut self, preset: Preset, area: Rect) {
        let ids = self.panes();
        self.zoom = None;
        if ids.len() < 2 {
            return;
        }
        let leaves = |ids: &[PaneId]| ids.iter().map(|&id| Node::Leaf(id)).collect::<Vec<_>>();
        let main_ratio = |cells: u16, total: u16| {
            let avail = f32::from(total.saturating_sub(1).max(2));
            (f32::from(cells.min(total * 2 / 3).max(1)) / avail).clamp(0.05, 0.95)
        };
        let (main, rest) = (Node::Leaf(ids[0]), &ids[1..]);
        self.root = match preset {
            Preset::EvenHorizontal => chain(leaves(&ids), Split::SideBySide),
            Preset::EvenVertical => chain(leaves(&ids), Split::Stacked),
            Preset::MainVertical | Preset::MainVerticalMirrored => {
                let r = main_ratio(MAIN_COLS, area.w);
                let others = Box::new(chain(leaves(rest), Split::Stacked));
                if preset == Preset::MainVertical {
                    Node::Branch { kind: Split::SideBySide, ratio: r, a: Box::new(main), b: others }
                } else {
                    Node::Branch { kind: Split::SideBySide, ratio: 1.0 - r, a: others, b: Box::new(main) }
                }
            }
            Preset::MainHorizontal | Preset::MainHorizontalMirrored => {
                let r = main_ratio(MAIN_ROWS, area.h);
                let others = Box::new(chain(leaves(rest), Split::SideBySide));
                if preset == Preset::MainHorizontal {
                    Node::Branch { kind: Split::Stacked, ratio: r, a: Box::new(main), b: others }
                } else {
                    Node::Branch { kind: Split::Stacked, ratio: 1.0 - r, a: others, b: Box::new(main) }
                }
            }
            Preset::Tiled => {
                let columns = (1..=ids.len()).find(|c| c * c >= ids.len()).unwrap_or(1);
                let rows: Vec<Node> = ids.chunks(columns).map(|row| chain(leaves(row), Split::SideBySide)).collect();
                chain(rows, Split::Stacked)
            }
        };
    }

    /// Give every pane its share again without changing the structure: each divider moves so
    /// that the panes on its two sides get room in proportion to how many panes they hold
    /// (`select-layout -E`).
    pub fn spread(&mut self) {
        spread_node(&mut self.root);
    }

    /// The tree with `leaf` applied to every pane id, in `panes()` order.
    pub fn shape<T>(&self, mut leaf: impl FnMut(PaneId) -> T) -> Shape<T> {
        shape_of(&self.root, &mut leaf)
    }

    /// A layout with the same splits as `shape`; `leaf` gives each leaf its pane id, called in
    /// `panes()` order.
    pub fn from_shape<T>(shape: Shape<T>, mut leaf: impl FnMut(T) -> PaneId) -> Layout {
        Layout { root: node_of(shape, &mut leaf), zoom: None }
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        leaves(&self.root, &mut out);
        out
    }

    /// Where each pane is; with a zoomed pane just that one, over the whole area.
    pub fn rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        if let Some(z) = self.zoomed() {
            return vec![(z, area)];
        }
        let (mut rects, mut seps) = (Vec::new(), Vec::new());
        collect(&self.root, area, &mut rects, &mut seps);
        rects
    }

    pub fn separators(&self, area: Rect) -> Vec<Separator> {
        if self.zoomed().is_some() {
            return Vec::new();
        }
        let (mut rects, mut seps) = (Vec::new(), Vec::new());
        collect(&self.root, area, &mut rects, &mut seps);
        seps
    }

    /// Split `target` in two, the new pane going right or below. Fails when too small.
    pub fn split(&mut self, target: PaneId, kind: Split, new: PaneId, area: Rect) -> Result<(), &'static str> {
        let rect = self.rects(area).into_iter().find(|(id, _)| *id == target).map(|(_, r)| r);
        let Some(rect) = rect else { return Err("no such pane") };
        let along = if kind == Split::SideBySide { rect.w } else { rect.h };
        if along < 3 {
            return Err("pane too small");
        }
        replace_leaf(&mut self.root, target, kind, new);
        Ok(())
    }

    /// Split the whole window in two (`split-window -f`): everything there is now becomes one half
    /// and `new` the other, below or to the right, or above or to the left with `before`.
    pub fn split_root(&mut self, kind: Split, new: PaneId, before: bool, area: Rect) -> Result<(), &'static str> {
        let along = if kind == Split::SideBySide { area.w } else { area.h };
        if along < 3 {
            return Err("window too small");
        }
        self.zoom = None;
        let old = std::mem::replace(&mut self.root, Node::Leaf(0));
        let (a, b) = if before { (Node::Leaf(new), old) } else { (old, Node::Leaf(new)) };
        self.root = Node::Branch { kind, ratio: 0.5, a: Box::new(a), b: Box::new(b) };
        Ok(())
    }

    /// Give `new`, the second half of a split just made, `cells` along the split. Both halves keep
    /// at least one cell, so a size that does not fit is cut to what does.
    pub fn size_second(&mut self, new: PaneId, kind: Split, cells: u16, area: Rect) {
        self.size_half(new, kind, cells, area, true);
    }

    /// The same for a new pane that is the first half (above or to the left of what it split).
    pub fn size_first(&mut self, new: PaneId, kind: Split, cells: u16, area: Rect) {
        self.size_half(new, kind, cells, area, false);
    }

    fn size_half(&mut self, new: PaneId, kind: Split, cells: u16, area: Rect, second: bool) {
        let Some(rect) = self.rects(area).into_iter().find(|(id, _)| *id == new).map(|(_, r)| r) else { return };
        let have = if kind == Split::SideBySide { rect.w } else { rect.h };
        let (shrink, grow) = match (kind, second) {
            (Split::SideBySide, true) => (Direction::Right, Direction::Left),
            (Split::Stacked, true) => (Direction::Down, Direction::Up),
            (Split::SideBySide, false) => (Direction::Left, Direction::Right),
            (Split::Stacked, false) => (Direction::Up, Direction::Down),
        };
        match cells.cmp(&have) {
            std::cmp::Ordering::Less => {
                self.resize(new, shrink, have - cells, area);
            }
            std::cmp::Ordering::Greater => {
                self.resize(new, grow, cells - have, area);
            }
            std::cmp::Ordering::Equal => {}
        }
    }

    /// Remove a pane, its sibling takes the space. False for the last pane.
    pub fn remove(&mut self, target: PaneId) -> bool {
        if !contains(&self.root, target) || matches!(self.root, Node::Leaf(_)) {
            return false;
        }
        let root = std::mem::replace(&mut self.root, Node::Leaf(0));
        match remove_leaf(root, target) {
            Some(node) => {
                self.root = node;
                if self.zoom == Some(target) {
                    self.zoom = None;
                }
                true
            }
            None => false,
        }
    }

    /// The pane across the border in `dir`, aligned with the middle of `from`'s edge.
    pub fn neighbor(&self, from: PaneId, dir: Direction, area: Rect) -> Option<PaneId> {
        let rects = self.rects(area);
        let r = rects.iter().find(|(id, _)| *id == from)?.1;
        let (x, y, w, h) = (i32::from(r.x), i32::from(r.y), i32::from(r.w), i32::from(r.h));
        let mid_x = x + w / 2;
        let mid_y = y + h / 2;
        let mut best: Option<(PaneId, i32)> = None;
        for (id, c) in &rects {
            let (cx, cy, cw, ch) = (i32::from(c.x), i32::from(c.y), i32::from(c.w), i32::from(c.h));
            let (touches, overlap, hit) = match dir {
                Direction::Right => (cx == x + w + 1, overlap(y, h, cy, ch), (cy..cy + ch).contains(&mid_y)),
                Direction::Left => (cx + cw + 1 == x, overlap(y, h, cy, ch), (cy..cy + ch).contains(&mid_y)),
                Direction::Down => (cy == y + h + 1, overlap(x, w, cx, cw), (cx..cx + cw).contains(&mid_x)),
                Direction::Up => (cy + ch + 1 == y, overlap(x, w, cx, cw), (cx..cx + cw).contains(&mid_x)),
            };
            if !touches || overlap == 0 {
                continue;
            }
            let score = overlap + if hit { 10_000 } else { 0 };
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((*id, score));
            }
        }
        best.map(|(id, _)| id)
    }

    /// The pane at an edge or corner of the window (`{top}`, `{bottom-left}` ...). An edge with
    /// no corner is the pane that reaches it, the leftmost or topmost of those; a corner is the
    /// pane that covers that cell.
    pub fn at_edge(&self, v: Option<Vert>, h: Option<Horiz>, area: Rect) -> Option<PaneId> {
        let rects = self.rects(area);
        let (min_x, min_y) = (rects.iter().map(|(_, r)| r.x).min()?, rects.iter().map(|(_, r)| r.y).min()?);
        let (max_x, max_y) = (rects.iter().map(|(_, r)| r.x + r.w).max()?, rects.iter().map(|(_, r)| r.y + r.h).max()?);
        let covers = |r: &Rect, x: u16, y: u16| r.x <= x && x < r.x + r.w && r.y <= y && y < r.y + r.h;
        match (v, h) {
            (Some(v), Some(h)) => {
                let x = if h == Horiz::Left { min_x } else { max_x.saturating_sub(1) };
                let y = if v == Vert::Top { min_y } else { max_y.saturating_sub(1) };
                rects.iter().find(|(_, r)| covers(r, x, y)).map(|(id, _)| *id)
            }
            (Some(Vert::Top), None) => rects.iter().filter(|(_, r)| r.y == min_y).min_by_key(|(_, r)| r.x).map(|(id, _)| *id),
            (Some(Vert::Bottom), None) => rects.iter().filter(|(_, r)| r.y + r.h == max_y).min_by_key(|(_, r)| r.x).map(|(id, _)| *id),
            (None, Some(Horiz::Left)) => rects.iter().filter(|(_, r)| r.x == min_x).min_by_key(|(_, r)| r.y).map(|(id, _)| *id),
            (None, Some(Horiz::Right)) => rects.iter().filter(|(_, r)| r.x + r.w == max_x).min_by_key(|(_, r)| r.y).map(|(id, _)| *id),
            (None, None) => None,
        }
    }

    /// Move the nearest divider around `target` one way: right/down grows the first child.
    pub fn resize(&mut self, target: PaneId, dir: Direction, cells: u16, area: Rect) -> bool {
        let (axis, delta) = match dir {
            Direction::Left => (Split::SideBySide, -i32::from(cells)),
            Direction::Right => (Split::SideBySide, i32::from(cells)),
            Direction::Up => (Split::Stacked, -i32::from(cells)),
            Direction::Down => (Split::Stacked, i32::from(cells)),
        };
        move_divider(&mut self.root, target, axis, delta, area) == Some(true)
    }

    /// Rebuild as one row or column of equally sized panes (`select-layout even-*`).
    pub fn make_even(&mut self, kind: Split) {
        let nodes: Vec<Node> = self.panes().into_iter().map(Node::Leaf).collect();
        if !nodes.is_empty() {
            self.root = chain(nodes, kind);
        }
    }
}

/// `nodes` side by side or stacked, each getting the same share.
fn chain(nodes: Vec<Node>, kind: Split) -> Node {
    let mut iter = nodes.into_iter().rev();
    let mut node = iter.next().unwrap_or(Node::Leaf(0));
    let mut count = 1;
    for next in iter {
        count += 1;
        node = Node::Branch { kind, ratio: 1.0 / count as f32, a: Box::new(next), b: Box::new(node) };
    }
    node
}

fn swap_ids(node: &mut Node, x: PaneId, y: PaneId) {
    match node {
        Node::Leaf(id) if *id == x => *id = y,
        Node::Leaf(id) if *id == y => *id = x,
        Node::Leaf(_) => {}
        Node::Branch { a, b, .. } => {
            swap_ids(a, x, y);
            swap_ids(b, x, y);
        }
    }
}

/// Put the ids of `order` into the leaves, left to right.
fn assign(node: &mut Node, order: &mut impl Iterator<Item = PaneId>) {
    match node {
        Node::Leaf(id) => {
            if let Some(next) = order.next() {
                *id = next;
            }
        }
        Node::Branch { a, b, .. } => {
            assign(a, order);
            assign(b, order);
        }
    }
}

/// Number of panes under `node`, after giving every branch below it a ratio by pane count.
fn spread_node(node: &mut Node) -> usize {
    match node {
        Node::Leaf(_) => 1,
        Node::Branch { ratio, a, b, .. } => {
            let (na, nb) = (spread_node(a), spread_node(b));
            *ratio = na as f32 / (na + nb) as f32;
            na + nb
        }
    }
}

fn overlap(a: i32, alen: i32, b: i32, blen: i32) -> i32 {
    ((a + alen).min(b + blen) - a.max(b)).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0, y: 0, w: 80, h: 23 };

    fn rect_of(l: &Layout, id: PaneId) -> Rect {
        l.rects(AREA).into_iter().find(|(p, _)| *p == id).unwrap().1
    }

    /// Every cell of the area belongs to exactly one pane or one separator.
    fn assert_tiles(l: &Layout, area: Rect) {
        let mut owner = vec![0u8; usize::from(area.w) * usize::from(area.h)];
        let mut mark = |x: u16, y: u16| {
            assert!(x < area.w && y < area.h, "cell ({x},{y}) outside area");
            owner[usize::from(y) * usize::from(area.w) + usize::from(x)] += 1;
        };
        for (_, r) in l.rects(area) {
            for y in r.y..r.y + r.h {
                for x in r.x..r.x + r.w {
                    mark(x, y);
                }
            }
        }
        for s in l.separators(area) {
            for i in 0..s.len {
                if s.vertical {
                    mark(s.x, s.y + i);
                } else {
                    mark(s.x + i, s.y);
                }
            }
        }
        assert!(owner.iter().all(|&n| n == 1), "cells not covered exactly once");
    }

    /// Pane 1 on the left; on the right 2 above 3 above 4... only 2 and 3 here: 1 | 2 / 3.
    fn left_and_two_right() -> Layout {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.split(2, Split::Stacked, 3, AREA).unwrap();
        l
    }

    #[test]
    fn edges_and_corners_name_the_pane_that_reaches_or_covers_them() {
        let l = left_and_two_right();
        assert_eq!(l.at_edge(Some(Vert::Top), None, AREA), Some(1), "leftmost of the panes at the top");
        assert_eq!(l.at_edge(Some(Vert::Bottom), None, AREA), Some(1));
        assert_eq!(l.at_edge(None, Some(Horiz::Left), AREA), Some(1));
        assert_eq!(l.at_edge(None, Some(Horiz::Right), AREA), Some(2), "topmost of the panes at the right");
        assert_eq!(l.at_edge(Some(Vert::Top), Some(Horiz::Right), AREA), Some(2));
        assert_eq!(l.at_edge(Some(Vert::Bottom), Some(Horiz::Right), AREA), Some(3));
        assert_eq!(l.at_edge(Some(Vert::Bottom), Some(Horiz::Left), AREA), Some(1));
        assert_eq!(l.at_edge(None, None, AREA), None);
        assert_eq!(Layout::new(7).at_edge(Some(Vert::Top), Some(Horiz::Left), AREA), Some(7));
    }

    #[test]
    fn the_second_half_of_a_split_can_be_given_a_size() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.size_second(2, Split::SideBySide, 20, AREA);
        assert_eq!(rect_of(&l, 2).w, 20);
        assert_eq!(rect_of(&l, 1).w, 59);
        l.size_second(2, Split::SideBySide, 70, AREA);
        assert_eq!(rect_of(&l, 2).w, 70);
        l.size_second(2, Split::SideBySide, 500, AREA);
        assert_eq!(rect_of(&l, 1).w, 1, "the first half keeps a cell");
        assert_tiles(&l, AREA);

        let mut s = Layout::new(1);
        s.split(1, Split::Stacked, 2, AREA).unwrap();
        s.size_second(2, Split::Stacked, 5, AREA);
        assert_eq!(rect_of(&s, 2).h, 5);
        assert_tiles(&s, AREA);
    }

    #[test]
    fn a_split_of_the_whole_window_spans_it_on_either_side() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.split_root(Split::Stacked, 3, false, AREA).unwrap();
        l.size_second(3, Split::Stacked, 5, AREA);
        assert_eq!((rect_of(&l, 3).w, rect_of(&l, 3).h, rect_of(&l, 3).y), (80, 5, 18));
        assert_tiles(&l, AREA);

        l.split_root(Split::SideBySide, 4, true, AREA).unwrap();
        l.size_first(4, Split::SideBySide, 10, AREA);
        assert_eq!((rect_of(&l, 4).x, rect_of(&l, 4).w, rect_of(&l, 4).h), (0, 10, 23));
        assert_tiles(&l, AREA);

        let mut tiny = Layout::new(1);
        assert!(tiny.split_root(Split::SideBySide, 2, false, Rect { x: 0, y: 0, w: 2, h: 5 }).is_err());
    }

    #[test]
    fn a_pane_from_another_window_takes_a_place() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        assert!(l.replace(2, 9));
        assert_eq!(l.panes(), vec![1, 9]);
        assert!(!l.replace(2, 7), "2 is gone");
        assert!(!l.replace(1, 9), "9 is here already");
    }

    #[test]
    fn single_pane_fills_the_area() {
        let l = Layout::new(1);
        assert_eq!(l.rects(AREA), vec![(1, AREA)]);
        assert!(l.separators(AREA).is_empty());
    }

    #[test]
    fn side_by_side_split_leaves_one_cell_divider() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        assert_eq!(rect_of(&l, 1), Rect { x: 0, y: 0, w: 40, h: 23 });
        assert_eq!(rect_of(&l, 2), Rect { x: 41, y: 0, w: 39, h: 23 });
        assert_eq!(l.separators(AREA), vec![Separator { x: 40, y: 0, len: 23, vertical: true }]);
        assert_tiles(&l, AREA);
    }

    #[test]
    fn stacked_split_leaves_one_cell_divider() {
        let mut l = Layout::new(1);
        l.split(1, Split::Stacked, 2, AREA).unwrap();
        assert_eq!(rect_of(&l, 1), Rect { x: 0, y: 0, w: 80, h: 11 });
        assert_eq!(rect_of(&l, 2), Rect { x: 0, y: 12, w: 80, h: 11 });
        assert_tiles(&l, AREA);
    }

    #[test]
    fn dragging_a_divider_moves_it_to_the_pointer_and_follows_it() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        let sep = l.separators(AREA)[0];
        let moved = l.drag_divider(sep, 20, AREA).unwrap();
        assert_eq!(moved, Separator { x: 20, ..sep });
        assert_eq!(rect_of(&l, 1).w, 20);
        let again = l.drag_divider(moved, 55, AREA).unwrap();
        assert_eq!(again.x, 55);
        assert_tiles(&l, AREA);
    }

    #[test]
    fn dragging_keeps_both_sides_at_least_one_cell() {
        let mut l = Layout::new(1);
        l.split(1, Split::Stacked, 2, AREA).unwrap();
        let sep = l.separators(AREA)[0];
        let top = l.drag_divider(sep, 0, AREA).unwrap();
        assert_eq!(rect_of(&l, 1).h, 1);
        let bottom = l.drag_divider(top, 500, AREA).unwrap();
        assert_eq!(rect_of(&l, 2).h, 1);
        assert_tiles(&l, AREA);
        assert_eq!(bottom.y, AREA.h - 2);
    }

    #[test]
    fn dragging_a_nested_divider_leaves_the_others_alone() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.split(2, Split::Stacked, 3, AREA).unwrap();
        let before = rect_of(&l, 1);
        let inner = *l.separators(AREA).iter().find(|s| !s.vertical).unwrap();
        l.drag_divider(inner, 5, AREA).unwrap();
        assert_eq!(rect_of(&l, 1), before);
        assert_eq!(rect_of(&l, 2).h, 5);
        assert_tiles(&l, AREA);
        assert!(l.drag_divider(Separator { x: 3, y: 3, len: 3, vertical: true }, 9, AREA).is_none());
    }

    #[test]
    fn nested_splits_tile_exactly_at_many_sizes() {
        for w in 5..60 {
            for h in 5..30 {
                let area = Rect { x: 0, y: 0, w, h };
                let mut l = Layout::new(1);
                l.split(1, Split::SideBySide, 2, area).unwrap();
                l.split(2, Split::Stacked, 3, area).unwrap();
                l.split(1, Split::Stacked, 4, area).unwrap();
                assert_tiles(&l, area);
            }
        }
    }

    #[test]
    fn split_refuses_tiny_panes() {
        let area = Rect { x: 0, y: 0, w: 2, h: 5 };
        let mut l = Layout::new(1);
        assert!(l.split(1, Split::SideBySide, 2, area).is_err());
        assert!(l.split(1, Split::Stacked, 2, area).is_ok());
    }

    #[test]
    fn remove_gives_space_to_the_sibling() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.split(2, Split::Stacked, 3, AREA).unwrap();
        assert!(l.remove(2));
        assert_eq!(l.panes(), vec![1, 3]);
        assert_eq!(rect_of(&l, 3).h, 23);
        assert_tiles(&l, AREA);
        assert!(l.remove(3));
        assert_eq!(l.rects(AREA), vec![(1, AREA)]);
        assert!(!l.remove(1));
    }

    #[test]
    fn neighbors_follow_geometry() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.split(2, Split::Stacked, 3, AREA).unwrap();
        assert_eq!(l.neighbor(1, Direction::Right, AREA), Some(2));
        assert_eq!(l.neighbor(2, Direction::Left, AREA), Some(1));
        assert_eq!(l.neighbor(3, Direction::Left, AREA), Some(1));
        assert_eq!(l.neighbor(2, Direction::Down, AREA), Some(3));
        assert_eq!(l.neighbor(3, Direction::Up, AREA), Some(2));
        assert_eq!(l.neighbor(1, Direction::Left, AREA), None);
        assert_eq!(l.neighbor(1, Direction::Up, AREA), None);
    }

    #[test]
    fn resize_moves_the_divider_and_clamps() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        assert!(l.resize(1, Direction::Right, 5, AREA));
        assert_eq!(rect_of(&l, 1).w, 45);
        assert_eq!(rect_of(&l, 2).w, 34);
        assert!(l.resize(2, Direction::Left, 3, AREA));
        assert_eq!(rect_of(&l, 1).w, 42);
        assert!(l.resize(1, Direction::Right, 500, AREA));
        assert_eq!(rect_of(&l, 2).w, 1);
        assert_tiles(&l, AREA);
        assert!(!l.resize(1, Direction::Down, 1, AREA));
    }

    #[test]
    fn resize_finds_the_ancestor_on_the_right_axis() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.split(2, Split::Stacked, 3, AREA).unwrap();
        assert!(l.resize(3, Direction::Up, 2, AREA));
        assert_eq!(rect_of(&l, 2).h, 9);
        assert!(l.resize(3, Direction::Right, 4, AREA));
        assert_eq!(rect_of(&l, 1).w, 44);
        assert_tiles(&l, AREA);
    }

    #[test]
    fn even_layouts_equalise() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.split(2, Split::SideBySide, 3, AREA).unwrap();
        l.resize(1, Direction::Right, 20, AREA);
        l.make_even(Split::Stacked);
        assert_eq!(l.panes(), vec![1, 2, 3]);
        let hs: Vec<u16> = l.rects(AREA).iter().map(|(_, r)| r.h).collect();
        assert!(hs.iter().all(|h| (6..=8).contains(h)), "{hs:?}");
        assert_tiles(&l, AREA);
        l.make_even(Split::SideBySide);
        let ws: Vec<u16> = l.rects(AREA).iter().map(|(_, r)| r.w).collect();
        assert!(ws.iter().all(|w| (25..=28).contains(w)), "{ws:?}");
        assert_tiles(&l, AREA);
    }

    #[test]
    fn a_layout_comes_apart_into_a_shape_and_is_built_again_with_the_same_tiling() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.split(2, Split::Stacked, 3, AREA).unwrap();
        l.resize(1, Direction::Right, 7, AREA);
        let shape = l.shape(|id| id * 10);
        assert_eq!(shape.leaf_count(), 3);
        let mut next = 100;
        let again = Layout::from_shape(shape.clone(), |old| {
            next += 1;
            assert!(old % 10 == 0);
            next
        });
        assert_eq!(again.panes(), vec![101, 102, 103], "ids are given in layout order");
        assert_eq!(again.shape(|_| ()), l.shape(|_| ()), "same splits and ratios");
        let (a, b): (Vec<Rect>, Vec<Rect>) = (l.rects(AREA).iter().map(|(_, r)| *r).collect(), again.rects(AREA).iter().map(|(_, r)| *r).collect());
        assert_eq!(a, b, "the same splits give the same rectangles");
        assert_tiles(&again, AREA);
    }

    #[test]
    fn a_shape_with_a_nonsense_ratio_still_builds_a_tiling_layout() {
        for ratio in [f32::NAN, f32::INFINITY, -3.0, 0.0, 1.0, 40.0] {
            let shape = Shape::Branch { kind: Split::Stacked, ratio, a: Box::new(Shape::Leaf(1)), b: Box::new(Shape::Leaf(2)) };
            let mut n = 0;
            let l = Layout::from_shape(shape, |_| {
                n += 1;
                n
            });
            assert_tiles(&l, AREA);
        }
    }

    #[test]
    fn shrinking_the_area_keeps_tiling() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, AREA).unwrap();
        l.split(2, Split::Stacked, 3, AREA).unwrap();
        for w in 3..80 {
            assert_tiles(&l, Rect { x: 0, y: 0, w, h: 7 });
        }
    }

    const BIG: Rect = Rect { x: 0, y: 0, w: 200, h: 50 };

    /// `n` panes numbered 1..=n, split alternately so the tree is not a plain row.
    fn many(n: u32) -> Layout {
        let mut l = Layout::new(1);
        for id in 2..=n {
            let kind = if id % 2 == 0 { Split::SideBySide } else { Split::Stacked };
            l.split(id - 1, kind, id, BIG).unwrap();
        }
        l
    }

    fn sorted(mut ids: Vec<PaneId>) -> Vec<PaneId> {
        ids.sort_unstable();
        ids
    }

    #[test]
    fn a_zoomed_pane_fills_the_area_alone_and_the_others_come_back() {
        let mut l = many(3);
        let before = l.rects(BIG);
        assert!(l.toggle_zoom(2));
        assert_eq!(l.zoomed(), Some(2));
        assert_eq!(l.rects(BIG), vec![(2, BIG)]);
        assert!(l.separators(BIG).is_empty());
        assert!(!l.toggle_zoom(2));
        assert_eq!(l.rects(BIG), before);
        assert!(!Layout::new(1).toggle_zoom(1), "one pane has nothing to zoom");
    }

    #[test]
    fn removing_the_zoomed_pane_ends_the_zoom() {
        let mut l = many(3);
        l.toggle_zoom(3);
        assert!(l.remove(3));
        assert_eq!(l.zoomed(), None);
        assert_tiles(&l, BIG);
    }

    #[test]
    fn swapping_exchanges_the_places_of_two_panes() {
        let mut l = many(3);
        let (r1, r3) = (rect_in(&l, 1), rect_in(&l, 3));
        assert!(l.swap(1, 3));
        assert_eq!((rect_in(&l, 1), rect_in(&l, 3)), (r3, r1));
        assert!(!l.swap(1, 1));
        assert!(!l.swap(1, 99));
        assert_tiles(&l, BIG);
    }

    fn rect_in(l: &Layout, id: PaneId) -> Rect {
        l.rects(BIG).into_iter().find(|(p, _)| *p == id).unwrap().1
    }

    #[test]
    fn rotating_moves_every_pane_one_place_and_wraps() {
        let mut l = many(3);
        let places: Vec<Rect> = l.rects(BIG).into_iter().map(|(_, r)| r).collect();
        l.rotate(false);
        assert_eq!(l.panes(), vec![2, 3, 1]);
        assert_eq!(l.rects(BIG).into_iter().map(|(_, r)| r).collect::<Vec<_>>(), places);
        l.rotate(true);
        l.rotate(true);
        assert_eq!(l.panes(), vec![3, 1, 2]);
        let mut one = Layout::new(7);
        one.rotate(true);
        assert_eq!(one.panes(), vec![7]);
    }

    #[test]
    fn every_preset_tiles_the_area_and_keeps_all_panes_in_order() {
        for preset in Preset::ALL {
            for n in 2..=8 {
                let mut l = many(n);
                let order = l.panes();
                l.apply(preset, BIG);
                assert_tiles(&l, BIG);
                if matches!(preset, Preset::MainVerticalMirrored | Preset::MainHorizontalMirrored) {
                    let panes = l.panes();
                    assert_eq!(panes.last(), Some(&order[0]), "the main pane comes last when mirrored");
                    assert_eq!(panes[..panes.len() - 1], order[1..]);
                } else {
                    assert_eq!(l.panes(), order, "{} with {n} panes", preset.name());
                }
                assert_eq!(sorted(l.rects(BIG).into_iter().map(|(p, _)| p).collect()), (1..=n).collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn the_main_panes_get_their_size_and_their_side() {
        let mut l = many(4);
        l.apply(Preset::MainVertical, BIG);
        let main = rect_in(&l, l.panes()[0]);
        assert_eq!((main.x, main.h), (0, BIG.h));
        assert!((79..=81).contains(&main.w), "{main:?}");
        l.apply(Preset::MainVerticalMirrored, BIG);
        let main = rect_in(&l, *l.panes().last().unwrap());
        assert_eq!(main.x + main.w, BIG.w);
        assert!((79..=81).contains(&main.w), "{main:?}");
        let mut l = many(4);
        l.apply(Preset::MainHorizontal, BIG);
        let main = rect_in(&l, l.panes()[0]);
        assert_eq!((main.y, main.w), (0, BIG.w));
        assert!((23..=25).contains(&main.h), "{main:?}");
        l.apply(Preset::MainHorizontalMirrored, BIG);
        let main = rect_in(&l, *l.panes().last().unwrap());
        assert_eq!(main.y + main.h, BIG.h);
    }

    #[test]
    fn a_main_pane_never_takes_more_than_two_thirds_of_a_small_window() {
        let mut l = many(3);
        l.apply(Preset::MainVertical, AREA);
        assert!(rect_of(&l, l.panes()[0]).w <= 54);
        assert_tiles(&l, AREA);
    }

    #[test]
    fn tiled_makes_a_grid() {
        let mut l = many(4);
        l.apply(Preset::Tiled, BIG);
        let widths: Vec<u16> = l.rects(BIG).iter().map(|(_, r)| r.w).collect();
        let heights: Vec<u16> = l.rects(BIG).iter().map(|(_, r)| r.h).collect();
        assert!(widths.iter().all(|w| (98..=100).contains(w)), "{widths:?}");
        assert!(heights.iter().all(|h| (24..=25).contains(h)), "{heights:?}");
        let mut five = many(5);
        five.apply(Preset::Tiled, BIG);
        assert_tiles(&five, BIG);
    }

    #[test]
    fn spreading_shares_the_room_again_without_changing_the_structure() {
        let mut l = many(3);
        l.drag_divider(l.separators(BIG)[0], 20, BIG);
        let order = l.panes();
        l.spread();
        assert_eq!(l.panes(), order);
        assert_tiles(&l, BIG);
        let rects = l.rects(BIG);
        let sizes: Vec<u16> = rects.iter().map(|(_, r)| r.w * r.h).collect();
        assert!(sizes.iter().max().unwrap() - sizes.iter().min().unwrap() < sizes.iter().sum::<u16>() / 2, "{sizes:?}");
    }

    #[test]
    fn preset_names_parse_and_cycle() {
        for p in Preset::ALL {
            assert_eq!(Preset::parse(p.name()), Some(p));
            assert_eq!(Preset::ALL[p.index()], p);
        }
        assert_eq!(Preset::parse("diagonal"), None);
    }
}
