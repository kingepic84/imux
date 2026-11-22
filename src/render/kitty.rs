//! Drawing image regions on a Kitty graphics client. Placements float over the text, so unlike
//! the other protocols they have to be deleted explicitly when the picture goes away.

use crate::image::{Bitmap, base64};

use super::iterm::region_png;

/// Payload bytes per escape sequence; the protocol allows at most 4096.
const CHUNK: usize = 4096;

/// Delete every placement on screen and free the data behind it.
pub const DELETE_ALL: &str = "\x1b_Ga=d,d=A,q=2\x1b\\";

/// Transmit and display cells `[tx, tx + w) x [ty, ty + h)` of an image spanning `cols` x `rows`
/// cells at the cursor, over exactly `w` x `h` cells and without moving the cursor. The renderer
/// uses `transmit_apc` and `place_apc` instead, which send the pixels once; this one-shot form is
/// kept for the tests that pin the encoding.
#[cfg(test)]
pub fn region_apc(b: &Bitmap, cols: usize, rows: usize, tile: (usize, usize), size: (usize, usize), cell: (usize, usize)) -> Result<String, String> {
    Ok(chunked(&format!("a=T,f=100,t=d,c={},r={},C=1,q=2,", size.0, size.1), &region_png(b, cols, rows, tile, size, cell)?))
}

/// Send the same crop as `region_apc` but only store it under `id` (`a=t`), to be shown with
/// `place_apc` any number of times without sending the pixels again.
pub fn transmit_apc(
    b: &Bitmap,
    cols: usize,
    rows: usize,
    tile: (usize, usize),
    size: (usize, usize),
    cell: (usize, usize),
    id: u32,
) -> Result<String, String> {
    Ok(chunked(&format!("a=t,i={id},f=100,t=d,q=2,"), &region_png(b, cols, rows, tile, size, cell)?))
}

/// Show the picture stored under `id` over `w` x `h` cells at the cursor, leaving the cursor.
pub fn place_apc(id: u32, w: usize, h: usize) -> String {
    format!("\x1b_Ga=p,i={id},c={w},r={h},C=1,q=2\x1b\\")
}

/// Store a whole picture (already a PNG) under `id`, to be shown in parts with `place_source_apc`.
pub fn transmit_png_apc(png: &[u8], id: u32) -> String {
    chunked(&format!("a=t,i={id},f=100,t=d,q=2,"), png)
}

/// Show the pixel rectangle `src` = (x, y, w, h) of the picture stored under `id` over `cells` =
/// (w, h) cells at the cursor, leaving the cursor. `placement` is a number of its own so that two
/// parts of one picture on a screen are two placements. No pixels travel: the terminal crops the
/// picture it holds, which is what keeps a scroll step cheap.
pub fn place_source_apc(id: u32, placement: u32, src: (usize, usize, usize, usize), cells: (usize, usize)) -> String {
    format!("\x1b_Ga=p,i={id},p={placement},x={},y={},w={},h={},c={},r={},C=1,q=2\x1b\\", src.0, src.1, src.2, src.3, cells.0, cells.1)
}

/// Remove one placement of the picture stored under `id`; the picture and its other placements stay.
pub fn delete_placement_apc(id: u32, placement: u32) -> String {
    format!("\x1b_Ga=d,d=i,i={id},p={placement},q=2\x1b\\")
}

/// Free the picture stored under `id` and every placement of it.
pub fn delete_image_apc(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// Remove every placement from the screen but keep the stored pictures for `place_apc`.
pub const DELETE_PLACEMENTS: &str = "\x1b_Ga=d,d=a,q=2\x1b\\";

/// The PNG as base64 in chunks; only the first carries `control` (which ends with a comma).
fn chunked(control: &str, png: &[u8]) -> String {
    let data = base64::encode(png);
    let chunks: Vec<&str> = data.as_bytes().chunks(CHUNK).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    let mut out = String::with_capacity(data.len() + 64 * chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            out.push_str(&format!("\x1b_G{control}m={more};{chunk}\x1b\\"));
        } else {
            // The terminal answers after the last chunk and may read the quiet setting from that
            // one: without `q` there it replies `ESC _ G i=0;OK`, which lands in the pane's input.
            out.push_str(&format!("\x1b_Gq=2,m={more};{chunk}\x1b\\"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(w: usize, h: usize) -> Bitmap {
        let mut b = Bitmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let n = (x * 2654435761 + y * 40503 + x * y * 97) >> 3;
                b.set(x, y, [n as u8, (n >> 8) as u8, (n >> 16) as u8, 255]);
            }
        }
        b
    }

    #[test]
    fn a_small_picture_is_one_sequence_that_sets_its_size_in_cells() {
        let apc = region_apc(&bitmap(4, 4), 2, 1, (0, 0), (2, 1), (2, 2)).unwrap();
        assert!(apc.starts_with("\x1b_Ga=T,f=100,t=d,c=2,r=1,C=1,q=2,m=0;"), "{apc:?}");
        assert!(apc.ends_with("\x1b\\"));
        assert_eq!(apc.matches("\x1b_G").count(), 1);
    }

    #[test]
    fn a_big_picture_is_split_into_chunks_that_decode_back_to_the_png() {
        let apc = region_apc(&bitmap(64, 64), 4, 4, (0, 0), (4, 4), (40, 40)).unwrap();
        let parts: Vec<&str> = apc.split("\x1b\\").filter(|p| !p.is_empty()).collect();
        assert!(parts.len() > 1, "{} parts", parts.len());
        let mut data = String::new();
        for (i, p) in parts.iter().enumerate() {
            let (control, payload) = p.strip_prefix("\x1b_G").unwrap().split_once(';').unwrap();
            assert!(payload.len() <= CHUNK);
            assert!(control.ends_with(if i + 1 < parts.len() { "m=1" } else { "m=0" }), "{control}");
            assert_eq!(control.starts_with("a=T"), i == 0);
            data.push_str(payload);
        }
        let img = crate::image::decode_image(&base64::decode(data.as_bytes()).unwrap()).unwrap();
        assert_eq!((img.w, img.h), (160, 160));
    }

    #[test]
    fn every_chunk_asks_the_terminal_not_to_answer() {
        let apc = transmit_png_apc(&vec![7u8; 20_000], 3);
        let parts: Vec<&str> = apc.split("\x1b\\").filter(|p| !p.is_empty()).collect();
        assert!(parts.len() > 2);
        for p in &parts {
            let control = p.strip_prefix("\x1b_G").unwrap().split_once(';').unwrap().0;
            assert!(control.split(',').any(|k| k == "q=2"), "{control}");
        }
    }

    #[test]
    fn a_stored_picture_is_sent_once_and_placed_by_id_without_pixels() {
        let apc = transmit_apc(&bitmap(4, 4), 2, 1, (0, 0), (2, 1), (2, 2), 7).unwrap();
        assert!(apc.starts_with("\x1b_Ga=t,i=7,f=100,t=d,q=2,m=0;"), "{apc:?}");
        let place = place_apc(7, 2, 1);
        assert_eq!(place, "\x1b_Ga=p,i=7,c=2,r=1,C=1,q=2\x1b\\");
        assert!(!place.contains(';'), "a placement carries no payload");
        assert!(DELETE_PLACEMENTS.contains("d=a") && !DELETE_PLACEMENTS.contains("d=A"), "data stays on the terminal");
    }

    #[test]
    fn a_part_of_a_stored_picture_is_placed_by_source_rectangle_and_carries_no_pixels() {
        let place = place_source_apc(7, 3, (10, 20, 30, 40), (6, 2));
        assert_eq!(place, "\x1b_Ga=p,i=7,p=3,x=10,y=20,w=30,h=40,c=6,r=2,C=1,q=2\x1b\\");
        assert!(!place.contains(';'));
        assert!(transmit_png_apc(b"png", 9).starts_with("\x1b_Ga=t,i=9,f=100,t=d,q=2,m=0;"));
        assert_eq!(delete_image_apc(9), "\x1b_Ga=d,d=I,i=9,q=2\x1b\\");
        assert_eq!(delete_placement_apc(9, 2), "\x1b_Ga=d,d=i,i=9,p=2,q=2\x1b\\");
    }

    #[test]
    fn deleting_names_the_action_and_frees_data() {
        assert!(DELETE_ALL.contains("a=d") && DELETE_ALL.contains("d=A"));
    }
}
