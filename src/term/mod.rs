pub mod cell;
mod clip;
pub mod grid;
mod images;
pub mod marks;
mod intercept;
#[cfg(test)]
mod kitty_extras;
mod oscguard;
mod parser;
mod placeholder;
#[cfg(test)]
mod real_programs;
mod resize;
pub mod status;

use grid::Grid;
use intercept::{Intercept, Piece};
use parser::Performer;

use std::collections::{HashMap, VecDeque};

use std::sync::Arc;
use std::time::Instant;

use crate::image::anim::{self, Animation};
use crate::image::iterm::Dim;
use crate::image::kitty::{self, Assembler};
use crate::image::worker::{self, Job};
use crate::image::{self, Bitmap, iterm};
use crate::log;

/// How deep `DCS tmux ;` passthroughs may nest before the inner one is left alone.
const MAX_WRAP_DEPTH: usize = 2;

/// Pictures a program stored with a Kitty `a=t` for a later `a=p`.
const MAX_STORED: usize = 16;

/// Bytes of pixels of those pictures together, and the biggest picture an animation starts from.
const MAX_STORED_BYTES: usize = 64 * 1024 * 1024;

/// Image numbers remembered (`I=`); the table starts over past this.
const MAX_NUMBERS: usize = 1024;

/// Animated pictures per pane.
const MAX_ANIMATIONS: usize = 8;

/// Placements remembered for deletes; older ones are forgotten (their cells stay).
const MAX_PLACEMENTS: usize = 1024;

/// Ids given to pictures that came with an image number only start here, out of the way of the
/// small numbers programs choose themselves.
const AUTO_ID_BASE: u32 = 0x8000_0000;

/// What a command that needs a picture says when none came (it cannot happen: the picture is
/// decoded before the command runs).
const NO_PICTURE: &str = "no picture";

/// One picture a program put on screen with an id, a number or a placement id.
#[derive(Clone, Copy, Debug)]
struct Placement {
    id: u32,
    number: u32,
    pid: u32,
    z: i32,
    image: image::store::ImageId,
    /// Shown by placeholder characters, not by cells stamped at the cursor.
    placeholders: bool,
}

/// What a held terminal is waiting to finish when the decoder answers.
enum Pending {
    /// An iTerm2 inline image whose data went to the decoder (`File::data` is empty).
    File(iterm::File),
    /// A Kitty command whose payload went to the decoder.
    Kitty(kitty::Command),
}

/// The request the terminal is waiting for.
struct Held {
    seq: u64,
    pending: Pending,
}

/// A pane's terminal: byte stream in, cell grid out. Replies (DA, CPR, ...) and events (title,
/// cwd, clipboard, bell) accumulate on the grid until the owner drains them.
pub struct Terminal {
    parser: vte::Parser,
    /// Keeps OSC strings short enough for the parser (which has no limit of its own).
    osc_guard: oscguard::OscGuard,
    intercept: Intercept,
    kitty: Assembler,
    stored: HashMap<u32, Bitmap>,
    /// Pictures put on screen with an id, a number or a placement id, so a delete can find them.
    placed: Vec<Placement>,
    /// Image numbers (`I=`) and the ids they were given, the latest transmission winning.
    numbers: HashMap<u32, u32>,
    /// The next id handed to a picture that came with a number and no id.
    next_auto_id: u32,
    /// Animated pictures by id (see `image/anim.rs`).
    animations: HashMap<u32, Animation>,
    /// An iTerm2 image arriving in parts (`imgcat`'s default): its arguments and the base64 so
    /// far. Kept here, not in the interceptor, because every part of a `DCS tmux ;` passthrough
    /// is wrapped, and so scanned, on its own.
    multipart: Option<(Vec<u8>, Vec<u8>)>,
    /// Where pictures are decoded; none decodes them on the calling thread (tests, popups).
    decoder: Option<worker::Handle>,
    /// A picture is being decoded: nothing after it is read until it is back, so the order of the
    /// pane's output stays what the program wrote.
    held: Option<Held>,
    /// What arrived while held, in order.
    queue: VecDeque<Piece<'static>>,
    /// Number of the last request to the decoder.
    seq: u64,
    /// Inside a `DCS tmux ;` passthrough: decoded in place, never held.
    nested: usize,
    pub grid: Grid,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize, history_limit: usize) -> Terminal {
        Terminal {
            parser: vte::Parser::new(),
            osc_guard: oscguard::OscGuard::default(),
            intercept: Intercept::default(),
            kitty: Assembler::default(),
            stored: HashMap::new(),
            placed: Vec::new(),
            numbers: HashMap::new(),
            next_auto_id: AUTO_ID_BASE,
            animations: HashMap::new(),
            multipart: None,
            decoder: None,
            held: None,
            queue: VecDeque::new(),
            seq: 0,
            nested: 0,
            grid: Grid::new(cols, rows, history_limit),
        }
    }

    /// Decode pictures on the worker behind `handle` instead of on the calling thread. The owner
    /// must call `finish_decode` with every answer and stop feeding while `is_held`.
    pub fn set_decoder(&mut self, handle: worker::Handle) {
        self.decoder = Some(handle);
    }

    /// A picture is being decoded and the output after it waits (see `held`). The owner should not
    /// read more from the program until `finish_decode` says it is over.
    pub fn is_held(&self) -> bool {
        self.held.is_some()
    }

    /// The decoder's answer to request `seq`. Finishes the held picture, then goes on with what
    /// waited behind it (which may hold again). Returns false for an answer nobody waits for.
    pub fn finish_decode(&mut self, seq: u64, result: Result<Bitmap, String>) -> bool {
        match self.held.take() {
            Some(held) if held.seq == seq => {
                self.resume(held.pending, result);
                let waiting = std::mem::take(&mut self.queue);
                self.run_pieces(waiting, 0);
                true
            }
            other => {
                self.held = other;
                false
            }
        }
    }

    /// Whether a picture may go to the decoder now.
    fn defers(&self) -> bool {
        self.decoder.is_some() && self.nested == 0
    }

    /// Send `job` to the decoder and wait for it, or decode it here when that is not possible.
    fn start(&mut self, pending: Pending, job: Job) {
        let Some(handle) = self.decoder.clone().filter(|_| self.nested == 0) else {
            return self.resume(pending, job.run());
        };
        self.seq += 1;
        let seq = self.seq;
        match handle.submit(seq, job) {
            Ok(()) => self.held = Some(Held { seq, pending }),
            Err(job) => self.resume(pending, job.run()),
        }
    }

    /// Carry on with a picture whose bitmap is there.
    fn resume(&mut self, pending: Pending, result: Result<Bitmap, String>) {
        match pending {
            Pending::File(file) => self.place_file(&file, result),
            Pending::Kitty(cmd) => self.kitty_run(cmd, Some(result)),
        }
    }

    /// `OSC 7501`: answer the query, or hand a valid report to the owner as an event.
    fn status_report(&mut self, body: &[u8]) {
        match status::parse(body) {
            Ok(status::Parsed::Query) => self.grid.reply(status::QUERY_REPLY),
            Ok(status::Parsed::Report(report)) => self.grid.events.push(grid::Event::Agent(report)),
            Err(e) => log::debug(&format!("status report ignored: {e}")),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut ic = std::mem::take(&mut self.intercept);
        let pieces = ic.split(bytes);
        self.intercept = ic;
        if self.held.is_some() {
            self.queue.extend(pieces.into_iter().map(Piece::into_owned));
        } else {
            self.run_pieces(pieces, 0);
        }
    }

    /// Carry out `pieces` in order. A picture handed to the decoder holds the terminal: what is
    /// left goes to the queue, to be run when the answer is back.
    fn run_pieces<'a>(&mut self, pieces: impl IntoIterator<Item = Piece<'a>>, depth: usize) {
        let mut rest = pieces.into_iter();
        while let Some(piece) = rest.next() {
            self.piece(piece, depth);
            if self.held.is_some() {
                self.queue.extend(rest.map(Piece::into_owned));
                return;
            }
        }
    }

    fn piece(&mut self, piece: Piece<'_>, depth: usize) {
        match piece {
            Piece::Pass(b) => {
                let b = self.osc_guard.pass(b);
                self.parser.advance(&mut Performer { grid: &mut self.grid }, &b);
            }
            Piece::Owned(b) => {
                let b = self.osc_guard.pass(&b);
                self.parser.advance(&mut Performer { grid: &mut self.grid }, &b);
            }
            Piece::File(payload) => self.show_file(&payload),
            Piece::Kitty(payload) => self.kitty_command(&payload),
            Piece::MultipartStart(args) => self.multipart = Some((args, Vec::new())),
            Piece::MultipartPart(data) => self.multipart_part(&data),
            Piece::MultipartEnd => self.multipart_end(),
            Piece::Status(body) => self.status_report(&body),
            Piece::Wrapped(inner) if depth < MAX_WRAP_DEPTH => {
                // Pictures inside a passthrough are decoded in place: its pieces are not kept.
                self.nested += 1;
                let mut ic = Intercept::default();
                let pieces = ic.split(&inner);
                self.run_pieces(pieces, depth + 1);
                self.nested -= 1;
            }
            Piece::Wrapped(_) => log::warn("passthrough envelope nested too deep, dropped"),
        }
    }

    /// One Kitty graphics command: query, transmit (and show), or show a stored picture.
    fn kitty_command(&mut self, raw: &[u8]) {
        let cmd = match self.kitty.push(raw) {
            Ok(Some(cmd)) => cmd,
            Ok(None) => return,
            Err(e) => {
                self.kitty.reset();
                return log::warn(&format!("kitty graphics: {e}"));
            }
        };
        self.kitty_run(cmd, None);
    }

    /// Carry out a whole Kitty command. `decoded` is the picture of a command that went to the
    /// decoder and came back (`resume`); the commands that need a picture hand it over first
    /// (when the decoder is there) and run again when it is back, so nothing below has happened
    /// before then.
    fn kitty_run(&mut self, mut cmd: kitty::Command, decoded: Option<Result<Bitmap, String>>) {
        let mut decoded = decoded;
        if decoded.is_none() && matches!(cmd.control.action, b'q' | b'T' | b't' | b'f') {
            if !self.defers() {
                decoded = Some(kitty::bitmap(&cmd));
            } else {
                match kitty::payload_owned(&mut cmd) {
                    Err(e) => decoded = Some(Err(e)),
                    Ok(payload) => {
                        let job = Job::Kitty { control: cmd.control.clone(), payload };
                        return self.start(Pending::Kitty(cmd), job);
                    }
                }
            }
        }
        let mut c = cmd.control.clone();
        if matches!(c.action, b'T' | b't') && c.number != 0 {
            if c.id == 0 {
                c.id = self.next_auto_id;
                self.next_auto_id = self.next_auto_id.wrapping_add(1).max(AUTO_ID_BASE);
            }
            if self.numbers.len() >= MAX_NUMBERS {
                self.numbers.clear();
            }
            self.numbers.insert(c.number, c.id);
        } else if c.id == 0 && c.number != 0 && c.action != b'd' {
            // Later commands may name the picture by its number.
            c.id = self.numbers.get(&c.number).copied().unwrap_or(0);
        }
        let result = match c.action {
            b'q' => decoded.take().unwrap_or_else(|| Err(NO_PICTURE.into())).map(drop),
            b'T' | b't' => decoded.take().unwrap_or_else(|| Err(NO_PICTURE.into())).map(|b| {
                if c.action == b'T' {
                    self.show_kitty(b, &c);
                } else {
                    // Stored pictures are outside the image store's budget, so they have their own
                    // (security review P-05: sixteen of 127 MiB each were 2 GiB per pane).
                    let held: usize = self.stored.values().map(Bitmap::bytes).sum();
                    if self.stored.len() >= MAX_STORED || held + b.bytes() > MAX_STORED_BYTES {
                        self.stored.clear();
                    }
                    if b.bytes() <= MAX_STORED_BYTES {
                        self.stored.insert(c.id, b);
                    }
                }
            }),
            b'p' => match self.stored.get(&c.id).cloned() {
                Some(b) => {
                    self.show_kitty(b, &c);
                    Ok(())
                }
                None => Err("no stored picture with that id".to_string()),
            },
            b'd' => return self.kitty_delete(&c),
            b'f' => self.kitty_frame(&cmd, c.id, decoded.take().unwrap_or_else(|| Err(NO_PICTURE.into()))),
            b'a' => self.kitty_animate(&c, c.id),
            b'c' => self.kitty_compose(&c, c.id),
            _ => return,
        };
        if let Err(e) = &result {
            log::warn(&format!("kitty graphics: {e}"));
        }
        let reply = kitty::reply(&c, result.err().as_deref());
        if !reply.is_empty() {
            self.grid.reply(&reply);
        }
    }

    /// Put a Kitty picture at the cursor; `c=` and `r=` give its size in cells, one of them
    /// keeps the aspect ratio, none uses the pixel size.
    fn show_kitty(&mut self, bitmap: Bitmap, c: &kitty::Control) {
        let dim = |n: usize| if n > 0 { Dim::Cells(n) } else { Dim::Auto };
        let preserve = c.cols == 0 || c.rows == 0;
        let (cols, rows) = iterm::footprint(bitmap.w, bitmap.h, dim(c.cols), dim(c.rows), preserve, self.grid.image_metrics());
        // Showing an id with a placement id again replaces that placement.
        if c.id != 0 && c.placement != 0 {
            self.remove_placements(|p| p.id == c.id && p.pid == c.placement);
        }
        let image = if c.unicode {
            // The program prints the placeholder characters itself: nothing is stamped now.
            let image = self.grid.images.add(bitmap, cols, rows);
            self.grid.add_virtual(c.id, image, cols, rows);
            self.grid.limit_images();
            image
        } else {
            self.grid.place_bitmap_at(bitmap, cols, rows, c.stay)
        };
        if c.id != 0 || c.number != 0 || c.placement != 0 {
            if self.placed.len() >= MAX_PLACEMENTS {
                self.placed.remove(0);
            }
            self.placed.push(Placement { id: c.id, number: c.number, pid: c.placement, z: c.z, image, placeholders: c.unicode });
        }
    }

    /// The animation of picture `id`: the one so far, or a new one that starts from the picture.
    fn animation(&mut self, id: u32) -> Result<&mut Animation, String> {
        if id == 0 {
            return Err("an animation needs the id of a picture".into());
        }
        if !self.animations.contains_key(&id) {
            if self.animations.len() >= MAX_ANIMATIONS {
                return Err("too many animated pictures".into());
            }
            let root = self
                .stored
                .get(&id)
                .cloned()
                .or_else(|| {
                    let placed = self.placed.iter().find(|p| p.id == id)?;
                    self.grid.images.image(placed.image).map(|p| (*p.bitmap).clone())
                })
                .ok_or("no picture with that id")?;
            if root.bytes() > MAX_STORED_BYTES {
                return Err("the picture is too big to animate".into());
            }
            self.animations.insert(id, Animation::new(Arc::new(root)));
        }
        self.animations.get_mut(&id).ok_or_else(|| "no picture with that id".to_string())
    }

    /// Show `pixels` for every placement of picture `id`.
    fn show_pixels(&mut self, id: u32, pixels: &Arc<Bitmap>) {
        for p in self.placed.iter().filter(|p| p.id == id) {
            self.grid.images.set_bitmap(p.image, pixels.clone());
        }
    }

    /// `a=f`: a frame for the animation of picture `id`: a new one, or `r=` to change one.
    fn kitty_frame(&mut self, cmd: &kitty::Command, id: u32, pixels: Result<Bitmap, String>) -> Result<(), String> {
        let c = &cmd.control;
        let pixels = pixels?;
        let anim = self.animation(id)?;
        let index = if c.rows > 0 {
            let i = c.rows - 1;
            if i >= anim.frames.len() {
                return Err("there is no such frame".into());
            }
            i
        } else {
            anim.add_frame((c.cols > 0).then(|| c.cols - 1), anim::color(c.y_up))?
        };
        anim.draw(index, &pixels, c.x, c.y, c.x_up == 1)?;
        if c.z != 0 {
            anim.set_gap(index, c.z)?;
        }
        let shown = (anim.current == index).then(|| anim.frames[index].bitmap.clone());
        if let Some(bitmap) = shown {
            self.show_pixels(id, &bitmap);
        }
        Ok(())
    }

    /// `a=a`: how the animation plays: the state (`s=`), the loops (`v=`), the frame shown
    /// (`c=`) and the gap of a frame (`r=` with `z=`).
    fn kitty_animate(&mut self, c: &kitty::Control, id: u32) -> Result<(), String> {
        let anim = self.animation(id)?;
        if c.z != 0 && c.rows > 0 {
            anim.set_gap(c.rows - 1, c.z)?;
        }
        let jump = (c.cols > 0).then(|| c.cols - 1);
        anim.control((c.width > 0).then_some(c.width as u32), c.height as u32, jump, Instant::now())?;
        let shown = jump.map(|j| anim.frames[j].bitmap.clone());
        if let Some(bitmap) = shown {
            self.show_pixels(id, &bitmap);
        }
        Ok(())
    }

    /// `a=c`: draw a rectangle of frame `r=` onto frame `c=`.
    fn kitty_compose(&mut self, c: &kitty::Control, id: u32) -> Result<(), String> {
        let anim = self.animation(id)?;
        let (Some(src), Some(dst)) = (c.rows.checked_sub(1), c.cols.checked_sub(1)) else {
            return Err("composing needs a source frame (r=) and a destination frame (c=)".into());
        };
        let source = anim.frames.get(src).ok_or("there is no such source frame")?.bitmap.clone();
        let part = anim::crop(&source, c.x_up as usize, c.y_up as usize, if c.w == 0 { source.w } else { c.w }, if c.h == 0 { source.h } else { c.h });
        anim.draw(dst, &part, c.x, c.y, c.stay)?;
        let shown = (anim.current == dst).then(|| anim.frames[dst].bitmap.clone());
        if let Some(bitmap) = shown {
            self.show_pixels(id, &bitmap);
        }
        Ok(())
    }

    /// `d=f`: the frames of picture `id` go; the picture stays as its first frame.
    fn drop_animation(&mut self, id: u32) {
        if let Some(anim) = self.animations.remove(&id) {
            self.show_pixels(id, &anim.frames[0].bitmap.clone());
        }
    }

    /// When the next animation frame is due.
    pub fn next_frame_due(&self) -> Option<Instant> {
        self.animations.values().filter_map(Animation::due).min()
    }

    /// Show the frames that are due at `now`. Returns whether any picture changed.
    pub fn advance_frames(&mut self, now: Instant) -> bool {
        let shown: Vec<(u32, Arc<Bitmap>)> = self.animations.iter_mut().filter_map(|(id, a)| Some((*id, a.tick(now)?))).collect();
        for (id, pixels) in &shown {
            self.show_pixels(*id, pixels);
        }
        !shown.is_empty()
    }

    /// Take the placements `hit` accepts off the screen and out of the list. Returns their ids.
    fn remove_placements(&mut self, hit: impl Fn(&Placement) -> bool) -> Vec<u32> {
        let gone: Vec<Placement> = self.placed.iter().filter(|p| hit(p)).copied().collect();
        self.drop_placements(&gone)
    }

    fn drop_placements(&mut self, gone: &[Placement]) -> Vec<u32> {
        let images: std::collections::HashSet<_> = gone.iter().map(|p| p.image).collect();
        let mut virtual_dropped = false;
        for p in gone.iter().filter(|p| p.placeholders) {
            virtual_dropped |= self.grid.drop_virtual(p.id).is_some();
        }
        self.grid.erase_images(|image| images.contains(&image));
        if virtual_dropped {
            self.grid.collect_images();
        }
        self.placed.retain(|p| !images.contains(&p.image));
        gone.iter().map(|p| p.id).collect()
    }

    /// `a=d`: take pictures off the screen. The target letter says which: `a` all, `i` an id
    /// (and a placement id when `p=` is given), `n` an image number, `c` those at the cursor,
    /// `p` at the cell (`x=`, `y=`), `q` at that cell with `z=`, `r` ids from `x=` to `y=`,
    /// `x` a column, `y` a row, `z` a stacking order. The upper-case forms also forget what was
    /// stored for a later `a=p`. Delete is never answered.
    fn kitty_delete(&mut self, c: &kitty::Control) {
        let free = c.delete.is_ascii_uppercase();
        let target = c.delete.to_ascii_lowercase();
        self.delete_targets(c, free, target);
        // An animation lives as long as its picture does.
        let (stored, placed) = (&self.stored, &self.placed);
        self.animations.retain(|id, _| stored.contains_key(id) || placed.iter().any(|p| p.id == *id));
    }

    fn delete_targets(&mut self, c: &kitty::Control, free: bool, target: u8) {
        if target == b'f' {
            return self.drop_animation(c.id);
        }
        if target == b'a' {
            self.grid.erase_images(|_| true);
            let virtual_ids: Vec<u32> = self.placed.iter().filter(|p| p.placeholders).map(|p| p.id).collect();
            for id in virtual_ids {
                self.grid.drop_virtual(id);
            }
            self.grid.collect_images();
            self.placed.clear();
            if free {
                self.stored.clear();
            }
            return;
        }
        let cell = |x: usize, y: usize| move |cx: usize, cy: usize| cx + 1 == x && cy + 1 == y;
        let at = |hit: &dyn Fn(usize, usize) -> bool, z: Option<i32>, this: &Terminal| -> Vec<Placement> {
            let seen = this.grid.images_where(hit);
            this.placed.iter().filter(|p| seen.contains(&p.image) && z.is_none_or(|z| p.z == z)).copied().collect()
        };
        let gone: Vec<Placement> = match target {
            b'i' => self.placed.iter().filter(|p| p.id == c.id && (c.placement == 0 || p.pid == c.placement)).copied().collect(),
            b'n' => {
                let id = self.numbers.get(&c.number).copied();
                self.placed.iter().filter(|p| p.number == c.number || Some(p.id) == id).copied().collect()
            }
            b'r' => self.placed.iter().filter(|p| p.id != 0 && (c.x..=c.y).contains(&(p.id as usize))).copied().collect(),
            b'z' => self.placed.iter().filter(|p| p.z == c.z).copied().collect(),
            b'c' => at(&cell(self.grid.cx + 1, self.grid.cy + 1), None, self),
            b'p' => at(&cell(c.x, c.y), None, self),
            b'q' => at(&cell(c.x, c.y), Some(c.z), self),
            b'x' => at(&|x, _| x + 1 == c.x, None, self),
            b'y' => at(&|_, y| y + 1 == c.y, None, self),
            _ => return,
        };
        let ids = self.drop_placements(&gone);
        if free {
            for id in ids {
                self.stored.remove(&id);
            }
            if target == b'i' {
                self.stored.remove(&c.id);
            }
        }
    }

    fn multipart_part(&mut self, data: &[u8]) {
        let Some((_, held)) = self.multipart.as_mut() else { return };
        if held.len() + data.len() > intercept::MAX_PAYLOAD {
            self.multipart = None;
            return log::warn(&format!("inline image dropped: payload over {} bytes", intercept::MAX_PAYLOAD));
        }
        held.extend_from_slice(data);
    }

    fn multipart_end(&mut self) {
        let Some((mut args, data)) = self.multipart.take() else { return };
        args.push(b':');
        args.extend_from_slice(&data);
        self.show_file(&args);
    }

    /// An iTerm2 `File=` image: decode it and put it at the cursor.
    fn show_file(&mut self, payload: &[u8]) {
        let mut file = match iterm::parse(payload) {
            Ok(f) => f,
            Err(e) => return log::warn(&format!("inline image: {e}")),
        };
        if !file.inline {
            return;
        }
        if self.defers() {
            let data = std::mem::take(&mut file.data);
            return self.start(Pending::File(file), Job::Any(data));
        }
        let result = image::decode_any(&file.data);
        self.place_file(&file, result);
    }

    /// Put a decoded iTerm2 image at the cursor.
    fn place_file(&mut self, file: &iterm::File, result: Result<Bitmap, String>) {
        match result {
            Ok(bitmap) => {
                let (cols, rows) =
                    iterm::footprint(bitmap.w, bitmap.h, file.width, file.height, file.preserve_aspect, self.grid.image_metrics());
                self.grid.place_bitmap(bitmap, cols, rows);
            }
            Err(e) => log::warn(&format!("inline image: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_b64() -> String {
        let mut out = Vec::new();
        let img = ::image::RgbaImage::from_pixel(20, 40, ::image::Rgba([10, 20, 30, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut out), ::image::ImageFormat::Png).unwrap();
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for chunk in out.chunks(3) {
            let n = chunk.iter().fold(0u32, |a, &b| (a << 8) | u32::from(b)) << (8 * (3 - chunk.len()));
            for i in 0..=chunk.len() {
                s.push(alphabet[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
            s.extend(std::iter::repeat_n('=', 3 - chunk.len()));
        }
        s
    }

    fn term() -> Terminal {
        let mut t = Terminal::new(20, 10, 100);
        t.grid.cell_px = (10, 20);
        t
    }

    #[test]
    fn an_endless_osc_string_cannot_grow_the_parser_and_the_pane_carries_on() {
        let mut t = term();
        t.feed(b"\x1b]52;c;");
        let chunk = vec![b'A'; 1 << 20];
        for _ in 0..8 {
            t.feed(&chunk);
        }
        t.feed(b"\x07ok");
        assert_eq!(t.grid.screen[0].text(), "ok");
        assert!(
            !t.grid.take_events().iter().any(|e| matches!(e, grid::Event::Clipboard { .. })),
            "a cut clipboard string is over the clipboard limit and refused"
        );
        t.feed(b"\x1b]52;c;Zm9v\x07");
        assert!(t.grid.take_events().iter().any(|e| matches!(e, grid::Event::Clipboard { .. })));
    }

    #[test]
    fn an_iterm2_image_lands_in_the_cells() {
        let mut t = term();
        t.feed(format!("hi\x1b]1337;File=inline=1;width=3:{}\x07after", png_b64()).as_bytes());
        let c = t.grid.screen[0].cells[2];
        let tile = t.grid.images.lookup(c.tile).expect("tile at the cursor");
        assert_eq!((tile.tx, tile.ty), (0, 0));
        assert_eq!(t.grid.images.image(tile.image).map(|p| (p.cols, p.rows)), Some((3, 3)));
        assert_eq!(t.grid.screen[0].text(), "hi");
        assert_eq!(t.grid.screen[3].cells[0].ch, 'a');
    }

    #[test]
    fn a_sixel_stream_sent_as_an_iterm2_file_lands_in_the_cells_too() {
        let mut t = term();
        let six = b"\x1bP0;1;0q\"1;1;20;12#0;2;100;0;0#0!20~!20~-\x1b\\";
        t.feed(format!("hi\x1b]1337;File=inline=1:{}\x07after", crate::image::base64::encode(six)).as_bytes());
        let tile = t.grid.images.lookup(t.grid.screen[0].cells[2].tile).expect("the picture is at the cursor");
        assert_eq!((tile.tx, tile.ty), (0, 0));
        assert!(t.grid.images.image(tile.image).is_some());
        assert_eq!(t.grid.screen[0].text(), "hi");
    }

    #[test]
    fn an_image_sent_in_parts_lands_in_the_cells_like_a_whole_one() {
        let mut t = term();
        let b64 = png_b64();
        let mut data = String::from("hi\x1b]1337;MultipartFile=inline=1;size=9;width=3\x07");
        for part in b64.as_bytes().chunks(8) {
            data.push_str(&format!("\x1b]1337;FilePart={}\x07", std::str::from_utf8(part).unwrap()));
        }
        data.push_str("\x1b]1337;FileEnd\x07after");
        for chunk in data.as_bytes().chunks(13) {
            t.feed(chunk);
        }
        assert_eq!(footprint_at(&t, 2, 0), Some((3, 3)));
        assert_eq!(t.grid.screen[3].cells[0].ch, 'a');
    }

    #[test]
    fn imgcat_in_a_tmux_like_term_wraps_every_part_on_its_own() {
        let mut t = term();
        let wrap = |s: String| format!("\x1bPtmux;{}\x1b\\", s.replace('\x1b', "\x1b\x1b"));
        let mut data = wrap("\x1b]1337;MultipartFile=inline=1;size=9\x07".to_string());
        for part in png_b64().as_bytes().chunks(200) {
            data.push_str(&wrap(format!("\x1b]1337;FilePart={}\x07", std::str::from_utf8(part).unwrap())));
        }
        data.push_str(&wrap("\x1b]1337;FileEnd\x07".to_string()));
        t.feed(data.as_bytes());
        assert!(t.grid.screen[0].cells[0].tile != 0);
    }

    #[test]
    fn a_stray_part_or_end_draws_nothing_and_a_broken_multipart_does_not_leak_into_the_next() {
        let mut t = term();
        t.feed(b"\x1b]1337;FilePart=AAAA\x07\x1b]1337;FileEnd\x07");
        assert_eq!(t.grid.images.tile_count(), 0);
        t.feed(b"\x1b]1337;MultipartFile=inline=1\x07\x1b]1337;FilePart=%%%\x07\x1b]1337;FileEnd\x07");
        assert_eq!(t.grid.images.tile_count(), 0);
        t.feed(format!("\x1b]1337;MultipartFile=inline=1\x07\x1b]1337;FilePart={}\x07\x1b]1337;FileEnd\x07", png_b64()).as_bytes());
        assert!(t.grid.screen[0].cells[0].tile != 0);
    }

    #[test]
    fn imgcat_style_tmux_wrapping_works() {
        let mut t = term();
        let inner = format!("\x1b]1337;File=inline=1:{}\x07", png_b64());
        let wrapped = format!("\x1bPtmux;{}\x1b\\", inner.replace('\x1b', "\x1b\x1b"));
        t.feed(wrapped.as_bytes());
        assert!(t.grid.screen[0].cells[0].tile != 0);
    }

    #[test]
    fn images_arrive_across_many_small_reads() {
        let mut t = term();
        let data = format!("\x1b]1337;File=inline=1:{}\x07", png_b64());
        for chunk in data.as_bytes().chunks(7) {
            t.feed(chunk);
        }
        assert!(t.grid.screen[0].cells[0].tile != 0);
    }

    #[test]
    fn a_sixel_string_lands_in_the_cells_even_when_split() {
        let mut t = term();
        let data = b"ab\x1bPq\"1;1;25;45#1;2;100;0;0#1!25~-!25~-!25~\x1b\\z";
        for chunk in data.chunks(5) {
            t.feed(chunk);
        }
        let tile = t.grid.images.lookup(t.grid.screen[0].cells[2].tile).expect("tile at the cursor");
        assert_eq!(t.grid.images.image(tile.image).map(|p| (p.cols, p.rows)), Some((3, 3)));
        assert_eq!(t.grid.screen[3].cells[0].ch, 'z');
    }

    fn footprint_at(t: &Terminal, x: usize, y: usize) -> Option<(usize, usize)> {
        let tile = t.grid.images.lookup(t.grid.screen[y].cells[x].tile)?;
        t.grid.images.image(tile.image).map(|p| (p.cols, p.rows))
    }

    #[test]
    fn a_kitty_png_in_chunks_lands_in_the_cells_with_the_asked_size() {
        let mut t = term();
        let b64 = png_b64();
        let (a, b) = b64.split_at(b64.len() / 2);
        let data = format!("hi\x1b_Ga=T,f=100,c=4,r=2,m=1;{a}\x1b\\\x1b_Gm=0;{b}\x1b\\after");
        for chunk in data.as_bytes().chunks(11) {
            t.feed(chunk);
        }
        assert_eq!(footprint_at(&t, 2, 0), Some((4, 2)));
        assert_eq!(t.grid.screen[0].text(), "hi");
        assert_eq!(t.grid.screen[2].cells[0].ch, 'a');
    }

    #[test]
    fn kitty_raw_pixels_and_the_support_query_get_answers() {
        let mut t = term();
        t.feed(b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
        assert_eq!(t.grid.take_replies(), b"\x1b_Gi=31;OK\x1b\\");
        assert_eq!(t.grid.images.tile_count(), 0, "a query shows nothing");
        t.feed(b"\x1b_Ga=T,f=24,s=1,v=1,c=3,r=1,q=2;/wAA\x1b\\");
        assert_eq!(footprint_at(&t, 0, 0), Some((3, 1)));
        assert!(t.grid.take_replies().is_empty());
    }

    #[test]
    fn a_stored_kitty_picture_can_be_placed_later() {
        let mut t = term();
        t.feed(b"\x1b_Ga=t,f=24,s=1,v=1,i=9;/wAA\x1b\\");
        assert_eq!(t.grid.images.tile_count(), 0);
        t.feed(b"\x1b_Ga=p,i=9,c=2,r=2\x1b\\");
        assert_eq!(footprint_at(&t, 1, 1), Some((2, 2)));
        assert_eq!(t.grid.take_replies(), b"\x1b_Gi=9;OK\x1b\\\x1b_Gi=9;OK\x1b\\");
        t.feed(b"\x1b_Ga=p,c=1\x1b\\");
        assert!(t.grid.take_replies().is_empty(), "no id to answer to means no answer");
        t.feed(b"\x1b_Ga=p,i=77\x1b\\");
        let reply = String::from_utf8(t.grid.take_replies()).unwrap();
        assert!(reply.starts_with("\x1b_Gi=77;EINVAL:"), "{reply:?}");
    }

    #[test]
    fn the_tables_a_program_can_fill_with_image_numbers_and_stored_pictures_have_ceilings() {
        let mut t = term();
        for n in 1..=3000u32 {
            t.feed(format!("\x1b_Ga=t,f=24,s=1,v=1,I={n},q=2;/wAA\x1b\\").as_bytes());
        }
        assert!(t.numbers.len() <= MAX_NUMBERS, "{}", t.numbers.len());
        assert!(t.stored.len() <= MAX_STORED);
    }

    fn shown(t: &Terminal, x: usize, y: usize) -> bool {
        t.grid.screen[y].cells[x].tile != 0
    }

    #[test]
    fn kitty_cursor_stays_put_with_c1_even_when_the_picture_scrolls() {
        let mut t = term();
        t.feed(b"ab\x1b_Ga=T,f=24,s=1,v=1,c=2,r=2,C=1,q=2;/wAA\x1b\\cd");
        assert!(shown(&t, 2, 1) && shown(&t, 3, 1));
        assert_eq!(t.grid.screen[0].text(), "abcd", "the cursor stayed, so the text went over the picture");
        let mut t = term();
        t.feed(b"\x1b[10;1Hxy\x1b_Ga=T,f=24,s=1,v=1,c=1,r=3,C=1,q=2;/wAA\x1b\\");
        assert_eq!((t.grid.cx, t.grid.cy), (2, 7), "two rows scrolled, so the cursor went up with the text");
        assert_eq!(t.grid.screen[7].text(), "xy");
        t.feed(b"z");
        assert_eq!(t.grid.screen[7].text(), "xyz");
    }

    #[test]
    fn kitty_delete_removes_by_id_or_all_and_upper_case_frees_the_stored_picture() {
        let mut t = term();
        t.feed(b"\x1b_Ga=T,f=24,s=1,v=1,i=1,q=2;/wAA\x1b\\\x1b_Ga=T,f=24,s=1,v=1,i=2,q=2;/wAA\x1b\\");
        assert!(shown(&t, 0, 0) && shown(&t, 0, 1));
        t.feed(b"\x1b_Ga=d,d=i,i=1\x1b\\");
        assert!(!shown(&t, 0, 0) && shown(&t, 0, 1));
        t.feed(b"\x1b_Ga=t,f=24,s=1,v=1,i=5,q=2;/wAA\x1b\\\x1b_Ga=d,d=i,i=5\x1b\\\x1b_Ga=p,i=5,q=2\x1b\\");
        assert!(shown(&t, 0, 2), "d=i keeps the stored picture");
        t.feed(b"\x1b_Ga=d,d=I,i=5\x1b\\\x1b_Ga=p,i=5\x1b\\");
        assert!(!shown(&t, 0, 2), "the placement is gone");
        assert!(!shown(&t, 0, 3), "d=I freed the stored picture, so this places nothing");
        assert!(String::from_utf8(t.grid.take_replies()).unwrap().contains("EINVAL"));
        t.feed(b"\x1b_Ga=d\x1b\\");
        assert!(!shown(&t, 0, 1));
        assert_eq!(t.grid.images.tile_count(), 0, "nothing shows any more, so nothing is kept");
    }

    #[test]
    fn kitty_delete_of_an_unknown_id_or_target_is_silent_and_harmless() {
        let mut t = term();
        t.feed(b"\x1b_Ga=T,f=24,s=1,v=1,i=1;/wAA\x1b\\");
        t.grid.take_replies();
        t.feed(b"\x1b_Ga=d,d=i,i=99\x1b\\\x1b_Ga=d,d=@\x1b\\\x1b_Ga=d,d=k,i=1\x1b\\");
        assert!(t.grid.take_replies().is_empty());
        assert!(shown(&t, 0, 0));
    }

    #[test]
    fn a_failed_kitty_command_answers_with_an_error_and_the_stream_goes_on() {
        let mut t = term();
        t.feed(b"\x1b_Ga=T,f=24,s=2,v=2,i=4;AAAA\x1b\\ok");
        let reply = String::from_utf8(t.grid.take_replies()).unwrap();
        assert!(reply.starts_with("\x1b_Gi=4;EINVAL:"), "{reply:?}");
        assert_eq!(t.grid.images.tile_count(), 0);
        assert_eq!(t.grid.screen[0].text(), "ok");
        t.feed(b"\x1b_Ga=T,s=x;AAAA\x1b\\\x1b_Gm=0;AAAA\x1b\\fine");
        assert!(t.grid.screen[0].text().ends_with("fine"));
    }

    #[test]
    fn a_broken_sixel_draws_nothing_and_the_stream_goes_on() {
        let mut t = term();
        t.feed(b"\x1bPq\x1b\\ok");
        assert_eq!(t.grid.images.tile_count(), 0);
        assert_eq!(t.grid.screen[0].text(), "ok");
    }

    #[test]
    fn a_download_or_broken_image_draws_nothing() {
        let mut t = term();
        t.feed(format!("\x1b]1337;File=inline=0:{}\x07", png_b64()).as_bytes());
        t.feed(b"\x1b]1337;File=inline=1:Zm9v\x07");
        t.feed(b"\x1b]1337;File=inline=1:%%%\x07");
        assert_eq!(t.grid.images.tile_count(), 0);
        assert_eq!((t.grid.cx, t.grid.cy), (0, 0));
    }

    /// What Claude Code 2.1.x sent in a captured session: a turn with a permission prompt.
    const CLAUDE_SESSION: &[u8] = b"\x1b]7501;state=idle:app=claude-code\x1b\\\
        \x1b]7501;state=working:app=claude-code\x1b\\\
        \x1b]7501;state=blocked:app=claude-code:kind=permission:msg=YXBwcm92ZSBCYXNoOiB0b3VjaCAvdG1wL2hvb2stZXhwL2RlbnlBLnR4dA==\x1b\\\
        \x1b]7501;state=working:app=claude-code\x1b\\\
        \x1b]7501;state=done:app=claude-code\x1b\\\
        \x1b]7501;state=clear\x1b\\";

    fn states(t: &mut Terminal) -> Vec<status::State> {
        t.grid
            .take_events()
            .into_iter()
            .filter_map(|e| match e {
                grid::Event::Agent(r) => Some(r.state),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_status_query_is_answered_and_a_report_becomes_an_event() {
        let mut t = term();
        t.feed(b"\x1b]7501;?\x07");
        assert_eq!(
            String::from_utf8(t.grid.take_replies()).unwrap(),
            "\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\"
        );
        t.feed(b"a\x1b]7501;state=working:app=x\x07b");
        assert_eq!(t.grid.screen[0].text(), "ab");
        assert_eq!(states(&mut t), [status::State::Working]);
    }

    #[test]
    fn a_session_of_reports_gives_the_same_events_wherever_reads_are_cut() {
        let mut whole = term();
        whole.feed(CLAUDE_SESSION);
        let expected = states(&mut whole);
        use status::State::*;
        assert_eq!(expected, [Idle, Working, Blocked, Working, Done, Clear]);
        for cut in 0..=CLAUDE_SESSION.len() {
            let mut t = term();
            t.feed(&CLAUDE_SESSION[..cut]);
            t.feed(&CLAUDE_SESSION[cut..]);
            assert_eq!(states(&mut t), expected, "cut at {cut}");
        }
        let mut t = term();
        for b in CLAUDE_SESSION {
            t.feed(&[*b]);
        }
        assert_eq!(states(&mut t), expected);
    }

    #[test]
    fn a_report_inside_a_tmux_passthrough_still_counts() {
        let mut t = term();
        t.feed(b"\x1bPtmux;\x1b\x1b]7501;state=idle\x07\x1b\\");
        assert_eq!(states(&mut t), [status::State::Idle]);
    }

    #[test]
    fn bad_reports_are_dropped_and_the_stream_goes_on() {
        let mut t = term();
        t.feed(b"x\x1b]7501;state=bogus\x07\x1b]7501;nonsense\x07\x1b]7501;state=working:msg=a-b_\x07y");
        assert_eq!(t.grid.screen[0].text(), "xy");
        assert!(states(&mut t).is_empty());
        let mut long = b"\x1b]7501;state=idle:app=".to_vec();
        long.extend(std::iter::repeat_n(b'a', 100_000));
        long.extend_from_slice(b"\x07z");
        t.feed(&long);
        assert_eq!(t.grid.screen[0].text(), "xyz");
        assert!(states(&mut t).is_empty());
    }

    #[test]
    fn a_shell_prompt_mark_is_an_event() {
        let mut t = term();
        t.feed(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;A;aid=1\x1b\\\x1b]133;D;0\x07");
        let starts = t.grid.take_events().into_iter().filter(|e| matches!(e, grid::Event::PromptStart)).count();
        assert_eq!(starts, 2);
    }
}

#[cfg(test)]
mod decoder_tests {
    //! Pictures decoded on the worker: the pane's output after a picture waits for it, and the
    //! result is the grid an in-place decode makes (security review: a 4 KB PNG that expands to
    //! 32 Mpx, sent 400 times, kept the single-threaded server from answering for half a minute).
    use super::*;
    use crate::image::worker::{self, Pool};
    use std::time::Duration;

    fn png_b64() -> String {
        let mut out = Vec::new();
        let img = ::image::RgbaImage::from_pixel(20, 40, ::image::Rgba([10, 20, 30, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut out), ::image::ImageFormat::Png).unwrap();
        crate::image::base64::encode(&out)
    }

    fn inline(width: usize) -> String {
        format!("\x1b]1337;File=inline=1;width={width}:{}\x07", png_b64())
    }

    fn plain() -> Terminal {
        let mut t = Terminal::new(20, 10, 100);
        t.grid.cell_px = (10, 20);
        t
    }

    fn deferred() -> (Terminal, Pool) {
        let pool = worker::spawn(|| {}).unwrap();
        let mut t = plain();
        t.set_decoder(pool.handle.with_tag(1));
        (t, pool)
    }

    /// Wait for the next answer and give it to the terminal.
    fn answer(t: &mut Terminal, pool: &Pool) -> bool {
        let done = pool.done.recv_timeout(Duration::from_secs(20)).expect("the decoder answers");
        t.finish_decode(done.seq, done.result)
    }

    /// What is on the screen: each row's text and which cells hold a picture tile.
    fn look(t: &Terminal) -> Vec<(String, Vec<bool>)> {
        t.grid.screen.iter().map(|r| (r.text(), r.cells.iter().map(|c| c.tile != 0).collect())).collect()
    }

    #[test]
    fn a_picture_holds_the_output_after_it_until_the_decoder_answers() {
        let bytes = format!("hi{}after", inline(3));
        let (mut t, pool) = deferred();
        t.feed(bytes.as_bytes());
        assert!(t.is_held(), "the picture went to the decoder");
        assert_eq!(t.grid.screen[0].text(), "hi", "what came before it is on the screen, what came after is not");
        assert_eq!(t.grid.images.tile_count(), 0);
        assert!(answer(&mut t, &pool));
        assert!(!t.is_held());
        let mut here = plain();
        here.feed(bytes.as_bytes());
        assert_eq!(look(&t), look(&here), "the same screen as decoding in place");
        assert_eq!((t.grid.cx, t.grid.cy), (here.grid.cx, here.grid.cy));
    }

    #[test]
    fn what_is_fed_while_held_waits_and_comes_out_in_order() {
        let (mut t, pool) = deferred();
        t.feed(inline(3).as_bytes());
        t.feed(b"X");
        t.feed(b"\x1b[1;1HY");
        assert!(t.is_held());
        assert_eq!(t.grid.screen[0].text(), "", "nothing was printed yet");
        assert!(answer(&mut t, &pool));
        let mut here = plain();
        here.feed(inline(3).as_bytes());
        here.feed(b"X");
        here.feed(b"\x1b[1;1HY");
        assert_eq!(look(&t), look(&here));
    }

    #[test]
    fn two_pictures_in_one_read_hold_twice() {
        let bytes = format!("{}m{}n", inline(2), inline(4));
        let (mut t, pool) = deferred();
        t.feed(bytes.as_bytes());
        assert!(answer(&mut t, &pool));
        assert!(t.is_held(), "the second picture is next");
        assert!(answer(&mut t, &pool));
        assert!(!t.is_held());
        let mut here = plain();
        here.feed(bytes.as_bytes());
        assert_eq!(look(&t), look(&here));
    }

    #[test]
    fn text_alone_never_holds() {
        let (mut t, _pool) = deferred();
        t.feed(b"plain \x1b[31mtext\x1b[0m\r\n");
        assert!(!t.is_held());
        assert_eq!(t.grid.screen[0].text(), "plain text");
    }

    #[test]
    fn an_answer_nobody_waits_for_is_ignored() {
        let (mut t, _pool) = deferred();
        assert!(!t.finish_decode(99, Err("late".into())));
        t.feed(inline(3).as_bytes());
        assert!(t.is_held());
        assert!(!t.finish_decode(t.seq + 1, Err("not ours".into())), "another request's answer");
        assert!(t.is_held(), "still waiting for its own");
    }

    #[test]
    fn a_picture_in_a_tmux_passthrough_is_decoded_in_place() {
        let (mut t, _pool) = deferred();
        let wrapped = format!("\x1bPtmux;{}\x1b\\", inline(3).replace('\x1b', "\x1b\x1b"));
        t.feed(wrapped.as_bytes());
        assert!(!t.is_held());
        assert!(t.grid.screen[0].cells[0].tile != 0);
    }

    #[test]
    fn a_picture_that_does_not_decode_is_dropped_and_the_pane_carries_on() {
        let (mut t, pool) = deferred();
        let junk = crate::image::base64::encode(b"this is not a picture at all");
        t.feed(format!("a\x1b]1337;File=inline=1:{junk}\x07b").as_bytes());
        assert!(t.is_held());
        assert!(answer(&mut t, &pool));
        assert!(!t.is_held());
        assert_eq!(t.grid.screen[0].text(), "ab");
        assert_eq!(t.grid.images.tile_count(), 0);
    }

    #[test]
    fn a_kitty_picture_goes_the_same_way() {
        let bytes = format!("k\x1b_Ga=T,f=100;{}\x1b\\z", png_b64());
        let (mut t, pool) = deferred();
        t.feed(bytes.as_bytes());
        assert!(t.is_held());
        assert!(answer(&mut t, &pool));
        let mut here = plain();
        here.feed(bytes.as_bytes());
        assert_eq!(look(&t), look(&here));
        assert!(t.grid.images.tile_count() > 0);
    }

    #[test]
    fn a_kitty_query_is_answered_only_once_the_picture_has_decoded() {
        let bytes = format!("\x1b_Ga=q,i=31,f=100;{}\x1b\\", png_b64());
        let (mut t, pool) = deferred();
        t.feed(bytes.as_bytes());
        assert!(t.grid.take_replies().is_empty(), "no answer before the decode");
        assert!(answer(&mut t, &pool));
        let reply = String::from_utf8_lossy(&t.grid.take_replies()).into_owned();
        assert!(reply.contains("i=31") && reply.contains("OK"), "{reply:?}");
    }

    #[test]
    fn a_picture_the_decoder_cannot_take_is_decoded_here() {
        let Pool { handle, done } = worker::spawn(|| {}).unwrap();
        drop(done);
        // The worker ends when it cannot deliver an answer, and its request channel closes.
        handle.with_tag(9).submit(1, worker::Job::Any(Vec::new())).ok().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let mut t = plain();
        t.set_decoder(handle.with_tag(1));
        t.feed(format!("{}x", inline(3)).as_bytes());
        assert!(!t.is_held(), "nobody to wait for");
        assert!(t.grid.screen[0].cells[0].tile != 0);
        assert_eq!(t.grid.screen[3].text(), "x");
    }
}
