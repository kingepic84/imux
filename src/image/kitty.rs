//! Kitty graphics protocol, input side: the commands a program in a pane sends. Only what is
//! needed to show a picture is understood: direct transmission (`t=d`) of PNG or raw RGB/RGBA,
//! in chunks and optionally zlib-compressed (`o=z`), to be displayed at once (`a=T`), stored
//! (`a=t`) or put on screen later (`a=p`), deleting (`a=d`, handled by the terminal) and the
//! support query (`a=q`). Everything else is answered with an error or ignored.

use super::{Bitmap, MAX_PIXELS, base64, decode_image};

/// Most base64 bytes taken in for one picture, the same cap as the other protocols.
const MAX_PAYLOAD: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub action: u8,
    pub format: u32,
    pub medium: u8,
    pub width: usize,
    pub height: usize,
    pub more: bool,
    pub id: u32,
    pub quiet: u8,
    pub cols: usize,
    pub rows: usize,
    pub compressed: bool,
    /// `d=`: what a delete (`a=d`) removes; lower case keeps the stored picture, upper frees it.
    pub delete: u8,
    /// `C=1`: leave the cursor where it was instead of moving it below the picture.
    pub stay: bool,
    /// `p=`: the placement id, so one image can be shown several times and each deleted alone.
    pub placement: u32,
    /// `I=`: the image number, a second name for an image that the terminal picks the id of.
    pub number: u32,
    /// `z=`: the stacking order of the placement, only used to delete by it.
    pub z: i32,
    /// `x=` and `y=`: a cell or a range for the delete targets that take one (1-based).
    pub x: usize,
    pub y: usize,
    /// `S=` and `O=`: how many bytes to read, and from where, of a file or shared memory.
    pub size: usize,
    pub offset: usize,
    /// `U=1`: the picture is shown by placeholder characters the program prints, not at the cursor.
    pub unicode: bool,
    /// `X=` and `Y=`: for a frame, how to compose it (1 overwrites) and the background color
    /// (0xRRGGBBAA); for `a=c`, the corner of the source rectangle.
    pub x_up: u32,
    pub y_up: u32,
    /// `w=` and `h=`: the size of a rectangle (`a=c`).
    pub w: usize,
    pub h: usize,
}

impl Default for Control {
    fn default() -> Control {
        Control {
            action: b't',
            format: 32,
            medium: b'd',
            width: 0,
            height: 0,
            more: false,
            id: 0,
            quiet: 0,
            cols: 0,
            rows: 0,
            compressed: false,
            delete: b'a',
            stay: false,
            placement: 0,
            number: 0,
            z: 0,
            x: 0,
            y: 0,
            size: 0,
            offset: 0,
            unicode: false,
            x_up: 0,
            y_up: 0,
            w: 0,
            h: 0,
        }
    }
}

/// A command whose chunks have all arrived.
#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub control: Control,
    /// The payload, base64 decoded.
    pub data: Vec<u8>,
}

fn parse_control(text: &str, base: Control) -> Result<Control, String> {
    let mut c = base;
    for pair in text.split(',').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').ok_or("control key without a value")?;
        let num = || value.parse::<u64>().map_err(|_| format!("bad number for {key}"));
        let ch = || value.bytes().next().ok_or_else(|| format!("empty value for {key}"));
        match key {
            "a" => c.action = ch()?,
            "f" => c.format = num()? as u32,
            "t" => c.medium = ch()?,
            "s" => c.width = num()?.min(1 << 20) as usize,
            "v" => c.height = num()?.min(1 << 20) as usize,
            "m" => c.more = num()? == 1,
            "i" => c.id = num()? as u32,
            "q" => c.quiet = num()?.min(2) as u8,
            "c" => c.cols = num()?.min(1 << 20) as usize,
            "r" => c.rows = num()?.min(1 << 20) as usize,
            "o" => c.compressed = ch()? == b'z',
            "d" => c.delete = ch()?,
            "C" => c.stay = num()? == 1,
            "p" => c.placement = num()? as u32,
            "I" => c.number = num()? as u32,
            "z" => c.z = value.parse::<i32>().map_err(|_| "bad number for z".to_string())?,
            "x" => c.x = num()?.min(1 << 20) as usize,
            "y" => c.y = num()?.min(1 << 20) as usize,
            "S" => c.size = num()?.min(MAX_PAYLOAD as u64) as usize,
            "O" => c.offset = num()?.min(u64::from(u32::MAX)) as usize,
            "U" => c.unicode = num()? == 1,
            "X" => c.x_up = num()?.min(u64::from(u32::MAX)) as u32,
            "Y" => c.y_up = num()?.min(u64::from(u32::MAX)) as u32,
            "w" => c.w = num()?.min(1 << 20) as usize,
            "h" => c.h = num()?.min(1 << 20) as usize,
            _ => {}
        }
    }
    Ok(c)
}

/// Collects the chunks of one transmission. Later chunks carry only `m` (and may repeat `i`, `q`).
#[derive(Default)]
pub struct Assembler {
    open: Option<(Control, Vec<u8>)>,
}

impl Assembler {
    /// Feed what followed `ESC _ G`. `Ok(None)` while more chunks are expected.
    pub fn push(&mut self, raw: &[u8]) -> Result<Option<Command>, String> {
        let (control_text, payload) = match raw.iter().position(|&b| b == b';') {
            Some(p) => (&raw[..p], &raw[p + 1..]),
            None => (raw, &raw[raw.len()..]),
        };
        let text = std::str::from_utf8(control_text).map_err(|_| "control data is not text")?;
        let (mut control, mut b64) = match self.open.take() {
            Some((c, d)) => (parse_control(text, c)?, d),
            None => (parse_control(text, Control::default())?, Vec::new()),
        };
        if b64.len() + payload.len() > MAX_PAYLOAD {
            return Err(format!("picture over {MAX_PAYLOAD} bytes"));
        }
        b64.extend_from_slice(payload);
        if control.more {
            control.more = false;
            let keep = Control { more: false, ..control };
            self.open = Some((keep, b64));
            return Ok(None);
        }
        let data = base64::decode(&b64).ok_or("payload is not base64")?;
        Ok(Some(Command { control, data }))
    }

    /// Forget a half-received transmission, e.g. after an error.
    pub fn reset(&mut self) {
        self.open = None;
    }
}

/// The payload, zlib-inflated when the command says `o=z`. Output over `limit` bytes is an
/// error, so a small payload cannot expand into all the memory.
fn inflated(data: &[u8], compressed: bool, limit: usize) -> Result<std::borrow::Cow<'_, [u8]>, String> {
    use std::borrow::Cow;
    if !compressed {
        return Ok(Cow::Borrowed(data));
    }
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit)
        .map(Cow::Owned)
        .map_err(|e| format!("payload does not inflate: {:?}", e.status))
}

/// The bytes of the picture a command carries: the payload itself (`t=d`, taken out of the
/// command), or what the file or shared memory the payload names holds (`t=f`, `t=t`, `t=s`, when
/// the option `kitty-media` allows it). This is the part that has to run on the server's thread
/// (it reads files and the option's setting); `decode_payload` is the heavy part and may run
/// anywhere.
pub fn payload_owned(cmd: &mut Command) -> Result<Vec<u8>, String> {
    let c = &cmd.control;
    match c.medium {
        b'd' => Ok(std::mem::take(&mut cmd.data)),
        b'f' | b't' | b's' => {
            // Raw pixels are exactly `s` x `v` of them; a shared memory object is rounded up to a
            // page, so without `S=` the size is taken from the picture.
            let bpp = if c.format == 24 { 3 } else { 4 };
            let sized = Control { size: if c.size == 0 && matches!(c.format, 24 | 32) { c.width * c.height * bpp } else { c.size }, ..c.clone() };
            super::media::read(&sized, &cmd.data)
        }
        other => Err(format!("transmission medium t={} is not supported", char::from(other))),
    }
}

/// The picture in `payload`, as the control data `c` says it is encoded: inflate it if it is
/// compressed, then decode a PNG, JPEG or GIF (`f=100`) or turn raw pixels (`f=24`, `f=32`) into
/// RGBA. Pure: it reads nothing but its arguments.
pub fn decode_payload(c: &Control, payload: &[u8]) -> Result<Bitmap, String> {
    match c.format {
        100 => decode_image(&inflated(payload, c.compressed, MAX_PAYLOAD)?),
        24 | 32 => {
            let bpp = if c.format == 24 { 3 } else { 4 };
            let (w, h) = (c.width, c.height);
            if w == 0 || h == 0 || w.saturating_mul(h) > MAX_PIXELS {
                return Err(format!("picture size {w}x{h} refused"));
            }
            let data = inflated(payload, c.compressed, w * h * bpp)?;
            if data.len() != w * h * bpp {
                return Err("the amount of pixel data does not fit s and v".to_string());
            }
            let mut rgba = Vec::with_capacity(w * h * 4);
            for px in data.chunks_exact(bpp) {
                rgba.extend_from_slice(&px[..3]);
                rgba.push(if bpp == 4 { px[3] } else { 255 });
            }
            Ok(Bitmap { w, h, rgba })
        }
        other => Err(format!("pixel format f={other} is not supported")),
    }
}

/// The picture a command carries (both steps, on this thread).
pub fn bitmap(cmd: &Command) -> Result<Bitmap, String> {
    if cmd.control.medium == b'd' {
        return decode_payload(&cmd.control, &cmd.data);
    }
    // For the other media `data` is only a name, so the copy is small.
    let mut named = Command { control: cmd.control.clone(), data: cmd.data.clone() };
    let payload = payload_owned(&mut named)?;
    decode_payload(&cmd.control, &payload)
}

/// The reply to a command: `ESC _ G i=<id> ; <message> ESC \`. Silent without an id, and as
/// `q` says (1 hides successes, 2 hides everything).
pub fn reply(c: &Control, error: Option<&str>) -> Vec<u8> {
    let silent = (c.id == 0 && c.number == 0) || c.quiet == 2 || (c.quiet == 1 && error.is_none());
    if silent {
        return Vec::new();
    }
    // Printable ASCII only: the answer is typed into the pane's input, and a pane-chosen byte in
    // it (a carriage return, or an escape that ends the string early) would be keystrokes for the
    // user's shell (security review P-03). Paths and sizes are not part of the message either.
    let msg = match error {
        None => "OK".to_string(),
        Some(e) => format!("EINVAL:{}", e.chars().map(|ch| if ch.is_ascii_graphic() || ch == ' ' { ch } else { '?' }).collect::<String>()),
    };
    let mut keys = Vec::new();
    if c.id != 0 {
        keys.push(format!("i={}", c.id));
    }
    if c.number != 0 {
        keys.push(format!("I={}", c.number));
    }
    if c.placement != 0 {
        keys.push(format!("p={}", c.placement));
    }
    format!("\x1b_G{};{msg}\x1b\\", keys.join(",")).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(raw: &[u8]) -> Command {
        Assembler::default().push(raw).unwrap().expect("complete")
    }

    #[test]
    fn a_single_command_has_its_keys_and_decoded_payload() {
        let c = complete(b"a=T,f=24,s=1,v=1,c=4,r=2,i=7,q=1;/wAA");
        assert_eq!(c.control.action, b'T');
        assert_eq!((c.control.format, c.control.width, c.control.height), (24, 1, 1));
        assert_eq!((c.control.cols, c.control.rows, c.control.id, c.control.quiet), (4, 2, 7, 1));
        assert_eq!(c.data, [255, 0, 0]);
    }

    #[test]
    fn chunks_are_joined_and_only_the_first_names_the_keys() {
        let mut a = Assembler::default();
        assert_eq!(a.push(b"a=T,f=24,s=2,v=1,m=1;/wAA"), Ok(None));
        assert_eq!(a.push(b"m=1;AP8A"), Ok(None));
        let done = a.push(b"m=0;").unwrap().expect("last chunk");
        assert_eq!(done.control.action, b'T');
        assert_eq!(done.data, [255, 0, 0, 0, 255, 0]);
        let px = bitmap(&done).unwrap();
        assert_eq!((px.w, px.h, px.pixel(1, 0)), (2, 1, [0, 255, 0, 255]));
    }

    #[test]
    fn rgba_keeps_its_alpha_and_rgb_is_opaque() {
        let rgba = bitmap(&complete(b"f=32,s=1,v=1;AQIDBA==")).unwrap();
        assert_eq!(rgba.pixel(0, 0), [1, 2, 3, 4]);
        let rgb = bitmap(&complete(b"f=24,s=1,v=1;AQID")).unwrap();
        assert_eq!(rgb.pixel(0, 0), [1, 2, 3, 255]);
    }

    #[test]
    fn png_payloads_are_decoded() {
        let mut png = Vec::new();
        let img = ::image::RgbaImage::from_pixel(3, 2, ::image::Rgba([9, 8, 7, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut png), ::image::ImageFormat::Png).unwrap();
        let raw = format!("a=T,f=100;{}", base64::encode(&png));
        let b = bitmap(&complete(raw.as_bytes())).unwrap();
        assert_eq!((b.w, b.h, b.pixel(2, 1)), (3, 2, [9, 8, 7, 255]));
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        assert!(Assembler::default().push(b"a=T,s=x;AAAA").is_err());
        assert!(Assembler::default().push(b"a=T;!!!!").is_err());
        assert!(Assembler::default().push(b"a=T,junk;AAAA").is_err());
        assert!(bitmap(&complete(b"f=24,s=2,v=2;AQID")).is_err(), "too few pixels");
        assert!(bitmap(&complete(b"f=24,s=0,v=1;AQID")).is_err());
        assert!(bitmap(&complete(b"f=99,s=1,v=1;AQID")).is_err());
        assert!(bitmap(&complete(b"f=24,s=1,v=1,t=f;AQID")).is_err(), "files are not read for a program");
        assert!(bitmap(&complete(b"f=24,s=1,v=1,o=z;AQID")).is_err());
        assert!(bitmap(&complete(b"f=100;AAAA")).is_err());
    }

    fn zlib_b64(raw: &[u8]) -> String {
        base64::encode(&miniz_oxide::deflate::compress_to_vec_zlib(raw, 6))
    }

    #[test]
    fn delete_and_cursor_keys_are_read() {
        let c = complete(b"a=d,d=I,i=3,C=1").control;
        assert_eq!((c.action, c.delete, c.id, c.stay), (b'd', b'I', 3, true));
        let c = complete(b"a=T").control;
        assert_eq!((c.delete, c.stay), (b'a', false));
    }

    #[test]
    fn zlib_payloads_are_inflated_for_raw_pixels_and_png() {
        let raw = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let cmd = complete(format!("f=32,s=2,v=1,o=z;{}", zlib_b64(&raw)).as_bytes());
        let b = bitmap(&cmd).unwrap();
        assert_eq!((b.w, b.h, b.pixel(1, 0)), (2, 1, [5, 6, 7, 8]));
        let mut png = Vec::new();
        let img = ::image::RgbaImage::from_pixel(3, 2, ::image::Rgba([9, 8, 7, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut png), ::image::ImageFormat::Png).unwrap();
        let b = bitmap(&complete(format!("f=100,o=z;{}", zlib_b64(&png)).as_bytes())).unwrap();
        assert_eq!((b.w, b.h, b.pixel(2, 1)), (3, 2, [9, 8, 7, 255]));
    }

    #[test]
    fn a_zlib_bomb_or_garbage_is_refused() {
        let bomb = zlib_b64(&vec![0u8; 4 << 20]);
        assert!(bitmap(&complete(format!("f=24,s=1,v=1,o=z;{bomb}").as_bytes())).is_err(), "inflates past the pixels asked for");
        assert!(bitmap(&complete(format!("f=24,s=1,v=1,o=z;{}", zlib_b64(&[1, 2])).as_bytes())).is_err(), "wrong size after inflating");
        assert!(bitmap(&complete(b"f=24,s=1,v=1,o=z;AAAAAAAA")).is_err(), "not zlib at all");
    }

    #[test]
    fn replies_follow_the_quiet_setting() {
        let c = |q, id| Control { id, quiet: q, ..Control::default() };
        assert_eq!(reply(&c(0, 5), None), b"\x1b_Gi=5;OK\x1b\\");
        assert!(reply(&c(1, 5), None).is_empty());
        assert!(reply(&c(1, 5), Some("x")).starts_with(b"\x1b_Gi=5;EINVAL:x"));
        assert!(reply(&c(2, 5), Some("x")).is_empty());
        assert!(reply(&c(0, 0), None).is_empty());
    }

    #[test]
    fn a_reply_never_carries_a_byte_the_pane_could_use_as_a_keystroke() {
        let c = Control { id: 1, ..Control::default() };
        for bad in ["\r", "\n", "\x1b\\", "\u{9b}", "\u{0}", "/\x1b\\\rtouch /tmp/x\r#"] {
            let r = reply(&c, Some(&format!("medium t={bad} x")));
            let body = &r[..r.len() - 2];
            assert!(body[2..].iter().all(|b| (0x20..0x7f).contains(b)), "{r:?}");
            assert!(r.ends_with(b"\x1b\\") && r.iter().filter(|&&b| b == 0x1b).count() == 2, "{r:?}");
        }
    }

    #[test]
    fn the_error_for_an_unknown_medium_is_refused_without_echoing_the_byte() {
        let err = bitmap(&complete(b"a=T,f=24,s=1,v=1,t=\r;AAAA")).unwrap_err();
        let r = reply(&Control { id: 1, ..Control::default() }, Some(&err));
        assert!(!r.contains(&b'\r'), "{r:?}");
    }
}
