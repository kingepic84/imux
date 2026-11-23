//! Kitty graphics animation: the frames of one image and when to show which. A program sends
//! further frames of a picture it has transmitted (`a=f`), says how to play them (`a=a`) and
//! may compose one frame out of parts of others (`a=c`). The server plays the animation itself:
//! each time a frame is due it swaps the picture's pixels, whatever the outer terminal is, so
//! a block, Sixel, iTerm2 or Kitty client sees the same thing (a Kitty client is sent the new
//! frame as a new picture, not as a frame of its own).

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::Bitmap;

/// Frames kept for one picture.
pub const MAX_FRAMES: usize = 64;

/// Pixels of all frames of one picture, in bytes.
pub const MAX_BYTES: usize = 32 * 1024 * 1024;

/// The gap of a frame that has none, and the shortest gap played (a program asking for less is
/// not slowing the whole server down).
pub const DEFAULT_GAP_MS: i32 = 40;
const MIN_GAP: Duration = Duration::from_millis(20);

#[derive(Clone)]
pub struct Frame {
    pub bitmap: Arc<Bitmap>,
    /// Milliseconds this frame stays; zero or less plays as fast as allowed.
    pub gap_ms: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Stopped,
    /// Playing, and waiting at the last frame for more instead of starting over.
    Loading,
    Running,
}

pub struct Animation {
    pub frames: Vec<Frame>,
    pub state: State,
    /// Wraps left after the last frame; `None` is for ever.
    loops: Option<u32>,
    pub current: usize,
    due: Option<Instant>,
}

/// `src` drawn over `dst` at (`x`, `y`): replacing the pixels, or blended by alpha. The part
/// that falls outside `dst` is cut off.
pub fn paint(dst: &mut Bitmap, src: &Bitmap, x: usize, y: usize, overwrite: bool) {
    for sy in 0..src.h {
        let dy = y + sy;
        if dy >= dst.h {
            break;
        }
        for sx in 0..src.w {
            let dx = x + sx;
            if dx >= dst.w {
                break;
            }
            let s = src.pixel(sx, sy);
            if overwrite || s[3] == 255 {
                dst.set(dx, dy, s);
            } else if s[3] != 0 {
                let d = dst.pixel(dx, dy);
                let a = u32::from(s[3]);
                let mix = |s: u8, d: u8| ((u32::from(s) * a + u32::from(d) * (255 - a)) / 255) as u8;
                let out_a = (a + u32::from(d[3]) * (255 - a) / 255).min(255) as u8;
                dst.set(dx, dy, [mix(s[0], d[0]), mix(s[1], d[1]), mix(s[2], d[2]), out_a]);
            }
        }
    }
}

/// The rectangle (`x`, `y`, `w`, `h`) of `src` as a bitmap of its own, cut at the edges.
pub fn crop(src: &Bitmap, x: usize, y: usize, w: usize, h: usize) -> Bitmap {
    let w = w.min(src.w.saturating_sub(x));
    let h = h.min(src.h.saturating_sub(y));
    let mut out = Bitmap::new(w, h);
    for oy in 0..h {
        for ox in 0..w {
            out.set(ox, oy, src.pixel(x + ox, y + oy));
        }
    }
    out
}

/// A color the way Kitty's `Y=` writes it: 0xRRGGBBAA.
pub fn color(rgba: u32) -> [u8; 4] {
    rgba.to_be_bytes()
}

impl Animation {
    /// An animation of one frame, the picture as it is.
    pub fn new(root: Arc<Bitmap>) -> Animation {
        Animation { frames: vec![Frame { bitmap: root, gap_ms: DEFAULT_GAP_MS }], state: State::Stopped, loops: None, current: 0, due: None }
    }

    pub fn bytes(&self) -> usize {
        self.frames.iter().map(|f| f.bitmap.bytes()).sum()
    }

    /// A new last frame that starts as a copy of frame `base` (0-based) or, without one, as a
    /// plain `background`. Returns its index.
    pub fn add_frame(&mut self, base: Option<usize>, background: [u8; 4]) -> Result<usize, String> {
        let size = &self.frames[0].bitmap;
        if self.frames.len() >= MAX_FRAMES {
            return Err(format!("an animation takes at most {MAX_FRAMES} frames"));
        }
        if self.bytes() + size.bytes() > MAX_BYTES {
            return Err("the animation would take too much memory".into());
        }
        let bitmap = match base {
            Some(b) => self.frames.get(b).ok_or("there is no such base frame")?.bitmap.clone(),
            None => {
                let mut solid = Bitmap::new(size.w, size.h);
                for px in solid.rgba.as_chunks_mut::<4>().0 {
                    px.copy_from_slice(&background);
                }
                Arc::new(solid)
            }
        };
        self.frames.push(Frame { bitmap, gap_ms: DEFAULT_GAP_MS });
        Ok(self.frames.len() - 1)
    }

    /// Draw `src` onto frame `index` at (`x`, `y`).
    pub fn draw(&mut self, index: usize, src: &Bitmap, x: usize, y: usize, overwrite: bool) -> Result<(), String> {
        let frame = self.frames.get_mut(index).ok_or("there is no such frame")?;
        paint(Arc::make_mut(&mut frame.bitmap), src, x, y, overwrite);
        Ok(())
    }

    pub fn set_gap(&mut self, index: usize, gap_ms: i32) -> Result<(), String> {
        self.frames.get_mut(index).ok_or("there is no such frame")?.gap_ms = gap_ms;
        Ok(())
    }

    /// `a=a`: change how the animation plays. `state` 1 stops it, 2 plays it and waits at the
    /// end for more frames, 3 plays it; `loops` 0 leaves it, 1 is for ever, `n` is `n - 1` wraps;
    /// `current` (0-based) jumps to a frame.
    pub fn control(&mut self, state: Option<u32>, loops: u32, current: Option<usize>, now: Instant) -> Result<(), String> {
        if let Some(c) = current {
            if c >= self.frames.len() {
                return Err("there is no such frame".into());
            }
            self.current = c;
        }
        match loops {
            0 => {}
            1 => self.loops = None,
            n => self.loops = Some(n - 1),
        }
        match state {
            Some(1) => (self.state, self.due) = (State::Stopped, None),
            Some(2) => (self.state, self.due) = (State::Loading, Some(now + self.gap(self.current))),
            Some(3) => (self.state, self.due) = (State::Running, Some(now + self.gap(self.current))),
            Some(_) => return Err("unknown animation state".into()),
            None => {}
        }
        Ok(())
    }

    fn gap(&self, index: usize) -> Duration {
        let ms = self.frames.get(index).map_or(DEFAULT_GAP_MS, |f| f.gap_ms);
        Duration::from_millis(ms.max(0) as u64).max(MIN_GAP)
    }

    /// When the next frame is due, if the animation plays.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// Move on if a frame is due. Returns the pixels to show when the picture changed.
    pub fn tick(&mut self, now: Instant) -> Option<Arc<Bitmap>> {
        if self.due.is_none_or(|d| d > now) || self.frames.len() < 2 {
            return None;
        }
        let mut next = self.current + 1;
        if next >= self.frames.len() {
            if self.state == State::Loading {
                self.due = None;
                return None;
            }
            match self.loops {
                Some(0) => {
                    (self.state, self.due) = (State::Stopped, None);
                    return None;
                }
                Some(n) => self.loops = Some(n - 1),
                None => {}
            }
            next = 0;
        }
        self.current = next;
        self.due = Some(now + self.gap(next));
        Some(self.frames[next].bitmap.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, px: [u8; 4]) -> Bitmap {
        let mut b = Bitmap::new(w, h);
        for p in b.rgba.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&px);
        }
        b
    }

    fn anim(frames: usize) -> Animation {
        let mut a = Animation::new(Arc::new(solid(2, 2, [1, 0, 0, 255])));
        for i in 1..frames {
            let at = a.add_frame(Some(0), [0; 4]).unwrap();
            a.draw(at, &solid(2, 2, [i as u8 + 1, 0, 0, 255]), 0, 0, true).unwrap();
            a.set_gap(at, 30).unwrap();
        }
        a
    }

    #[test]
    fn painting_blends_by_alpha_or_overwrites_and_is_cut_at_the_edge() {
        let mut dst = solid(2, 1, [100, 100, 100, 255]);
        paint(&mut dst, &solid(1, 1, [200, 0, 0, 128]), 0, 0, false);
        assert_eq!(dst.pixel(0, 0)[0], 150);
        assert_eq!(dst.pixel(1, 0), [100, 100, 100, 255]);
        paint(&mut dst, &solid(3, 3, [9, 9, 9, 10]), 1, 0, true);
        assert_eq!(dst.pixel(1, 0), [9, 9, 9, 10], "overwritten as it is, and the rest cut off");
        let mut dst = solid(1, 1, [5, 5, 5, 255]);
        paint(&mut dst, &solid(1, 1, [200, 0, 0, 0]), 0, 0, false);
        assert_eq!(dst.pixel(0, 0), [5, 5, 5, 255], "fully transparent changes nothing");
    }

    #[test]
    fn a_new_frame_starts_as_a_copy_or_as_the_background() {
        let mut a = Animation::new(Arc::new(solid(2, 2, [7, 7, 7, 255])));
        let copy = a.add_frame(Some(0), [0; 4]).unwrap();
        let plain = a.add_frame(None, color(0x11223344)).unwrap();
        assert_eq!(a.frames[copy].bitmap.pixel(1, 1), [7, 7, 7, 255]);
        assert_eq!(a.frames[plain].bitmap.pixel(1, 1), [0x11, 0x22, 0x33, 0x44]);
        assert!(a.add_frame(Some(9), [0; 4]).is_err());
    }

    #[test]
    fn editing_a_frame_does_not_change_the_one_it_was_copied_from() {
        let mut a = anim(2);
        a.draw(1, &solid(1, 1, [99, 0, 0, 255]), 1, 1, true).unwrap();
        assert_eq!(a.frames[0].bitmap.pixel(1, 1), [1, 0, 0, 255]);
        assert_eq!(a.frames[1].bitmap.pixel(1, 1), [99, 0, 0, 255]);
    }

    #[test]
    fn frames_and_memory_are_limited() {
        let mut a = Animation::new(Arc::new(solid(1, 1, [0; 4])));
        for _ in 1..MAX_FRAMES {
            a.add_frame(Some(0), [0; 4]).unwrap();
        }
        assert!(a.add_frame(Some(0), [0; 4]).is_err());
    }

    #[test]
    fn a_running_animation_steps_on_time_and_wraps() {
        let mut a = anim(3);
        let t0 = Instant::now();
        a.control(Some(3), 0, None, t0).unwrap();
        assert!(a.tick(t0).is_none(), "not yet due");
        let shown = |a: &mut Animation, t: Instant| a.tick(t).map(|b| b.pixel(0, 0)[0]);
        let t1 = t0 + Duration::from_millis(41);
        assert_eq!(shown(&mut a, t1), Some(2));
        let t2 = t1 + Duration::from_millis(31);
        assert_eq!(shown(&mut a, t2), Some(3));
        let t3 = t2 + Duration::from_millis(31);
        assert_eq!(shown(&mut a, t3), Some(1), "back to the first frame");
    }

    #[test]
    fn the_loop_count_stops_it_and_loading_waits_at_the_end() {
        let t0 = Instant::now();
        let mut a = anim(2);
        a.control(Some(3), 2, None, t0).unwrap();
        let mut t = t0;
        let mut seen = Vec::new();
        for _ in 0..6 {
            t += Duration::from_millis(50);
            if let Some(b) = a.tick(t) {
                seen.push(b.pixel(0, 0)[0]);
            }
        }
        assert_eq!(seen, vec![2, 1, 2], "v=2 is one wrap");
        assert_eq!(a.state, State::Stopped);
        let mut b = anim(2);
        b.control(Some(2), 0, None, t0).unwrap();
        assert!(b.tick(t0 + Duration::from_millis(50)).is_some());
        assert!(b.tick(t0 + Duration::from_millis(200)).is_none(), "waits for more frames");
        assert!(b.due().is_none());
    }

    #[test]
    fn stopping_and_jumping_work_and_a_bad_frame_is_an_error() {
        let t0 = Instant::now();
        let mut a = anim(3);
        a.control(Some(3), 0, Some(2), t0).unwrap();
        assert_eq!(a.current, 2);
        a.control(Some(1), 0, None, t0).unwrap();
        assert!(a.due().is_none());
        assert!(a.control(None, 0, Some(9), t0).is_err());
        assert!(a.control(Some(7), 0, None, t0).is_err());
    }

    #[test]
    fn a_very_short_gap_is_played_no_faster_than_the_minimum() {
        let mut a = anim(2);
        a.set_gap(0, -1).unwrap();
        let t0 = Instant::now();
        a.control(Some(3), 0, None, t0).unwrap();
        assert!(a.due().unwrap() >= t0 + MIN_GAP);
    }

    #[test]
    fn a_rectangle_can_be_cut_out_of_a_frame() {
        let mut src = solid(4, 4, [0, 0, 0, 255]);
        src.set(2, 1, [9, 9, 9, 255]);
        let part = crop(&src, 2, 1, 5, 5);
        assert_eq!((part.w, part.h), (2, 3));
        assert_eq!(part.pixel(0, 0), [9, 9, 9, 255]);
    }
}
