//! Tests of the Kitty graphics commands beyond the basics: delete targets, placement ids and
//! image numbers, media, unicode placeholders and animation, all through the byte stream.

use std::time::{Duration, Instant};

use super::Terminal;
use crate::image::media::{self, Level};

const RED: &str = "/wAA";
const GREEN: &str = "AP8A";
const BLUE: &str = "AAD/";

fn term() -> Terminal {
    let mut t = Terminal::new(20, 10, 100);
    t.grid.cell_px = (10, 20);
    t
}

fn shown(t: &Terminal, x: usize, y: usize) -> bool {
    t.grid.screen[y].cells[x].tile != 0
}

fn replies(t: &mut Terminal) -> String {
    String::from_utf8(t.grid.take_replies()).unwrap()
}

/// A 1x1 picture shown in one cell at the cursor.
fn put(t: &mut Terminal, keys: &str, data: &str) {
    t.feed(format!("\x1b_Ga=T,f=24,s=1,v=1,c=1,r=1,q=2,{keys};{data}\x1b\\").as_bytes());
}

fn pixel_at(t: &Terminal, x: usize, y: usize) -> [u8; 4] {
    let tile = t.grid.images.lookup(t.grid.screen[y].cells[x].tile).expect("a picture cell");
    t.grid.images.image(tile.image).expect("a picture").bitmap.pixel(0, 0)
}

#[test]
fn delete_by_position_column_row_and_cursor() {
    let mut t = term();
    put(&mut t, "i=1", RED);
    t.feed(b"\x1b[3;5H");
    put(&mut t, "i=2", GREEN);
    t.feed(b"\x1b_Ga=d,d=p,x=1,y=1\x1b\\");
    assert!(!shown(&t, 0, 0) && shown(&t, 4, 2), "the cell (1,1) held the first");
    t.feed(b"\x1b_Ga=d,d=x,x=5\x1b\\");
    assert!(!shown(&t, 4, 2), "column 5");
    put(&mut t, "i=3", BLUE);
    t.feed(b"\x1b[6;1H");
    put(&mut t, "i=4", RED);
    t.feed(b"\x1b_Ga=d,d=y,y=6\x1b\\");
    assert!(!shown(&t, 0, 5), "row 6");
    t.feed(b"\x1b[1;1H");
    put(&mut t, "i=5", RED);
    t.feed(b"\x1b[1;1H\x1b_Ga=d,d=c\x1b\\");
    assert!(!shown(&t, 0, 0), "the cell under the cursor");
}

#[test]
fn delete_by_z_range_and_number() {
    let mut t = term();
    put(&mut t, "i=10,z=-3", RED);
    t.feed(b"\x1b[2;1H");
    put(&mut t, "i=11,z=4", GREEN);
    t.feed(b"\x1b[3;1H");
    put(&mut t, "i=12", BLUE);
    t.feed(b"\x1b_Ga=d,d=z,z=4\x1b\\");
    assert!(shown(&t, 0, 0) && !shown(&t, 0, 1) && shown(&t, 0, 2), "only z=4 goes");
    t.feed(b"\x1b_Ga=d,d=r,x=10,y=10\x1b\\");
    assert!(!shown(&t, 0, 0) && shown(&t, 0, 2), "ids 10..10");
    t.feed(b"\x1b[4;1H");
    put(&mut t, "I=77", RED);
    assert!(shown(&t, 0, 3));
    t.feed(b"\x1b_Ga=d,d=n,I=77\x1b\\");
    assert!(!shown(&t, 0, 3), "by number");
}

#[test]
fn delete_by_cell_and_z_needs_both() {
    let mut t = term();
    put(&mut t, "i=1,z=2", RED);
    t.feed(b"\x1b_Ga=d,d=q,x=1,y=1,z=9\x1b\\");
    assert!(shown(&t, 0, 0), "another z");
    t.feed(b"\x1b_Ga=d,d=q,x=1,y=1,z=2\x1b\\");
    assert!(!shown(&t, 0, 0));
}

#[test]
fn a_placement_id_names_one_of_several_placements_of_a_stored_picture() {
    let mut t = term();
    t.feed(format!("\x1b_Ga=t,f=24,s=1,v=1,i=9,q=2;{RED}\x1b\\").as_bytes());
    t.feed(b"\x1b_Ga=p,i=9,p=1,q=2\x1b\\\x1b[2;1H\x1b_Ga=p,i=9,p=2,q=2\x1b\\");
    assert!(shown(&t, 0, 0) && shown(&t, 0, 1));
    t.feed(b"\x1b_Ga=d,d=i,i=9,p=1\x1b\\");
    assert!(!shown(&t, 0, 0) && shown(&t, 0, 1), "only placement 1");
    t.feed(b"\x1b[3;1H\x1b_Ga=p,i=9,p=2,q=2\x1b\\");
    assert!(!shown(&t, 0, 1) && shown(&t, 0, 2), "the same ids again replace the placement");
    t.feed(b"\x1b_Ga=d,d=i,i=9\x1b\\");
    assert!(!shown(&t, 0, 2), "without p= all placements of the id");
}

#[test]
fn a_picture_with_a_number_gets_an_id_and_can_be_found_by_the_number() {
    let mut t = term();
    t.feed(format!("\x1b_Ga=t,f=24,s=1,v=1,I=5;{RED}\x1b\\").as_bytes());
    let r = replies(&mut t);
    assert!(r.contains("I=5") && r.contains(";OK"), "{r:?}");
    assert!(r.contains("i=") && !r.contains("i=0"), "an id was handed out: {r:?}");
    t.feed(b"\x1b_Ga=p,I=5,q=2\x1b\\");
    assert!(shown(&t, 0, 0), "a=p by number");
}

#[test]
fn files_and_shared_memory_are_refused_until_the_option_allows_them() {
    let dir = std::env::temp_dir().join(format!("imux-kitty-media-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pixel.rgb");
    std::fs::write(&path, [255u8, 0, 0]).unwrap();
    let name = crate::image::base64::encode(path.to_str().unwrap().as_bytes());
    let mut t = term();
    media::set_level(Level::None);
    t.feed(format!("\x1b_Ga=T,t=f,f=24,s=1,v=1,i=1;{name}\x1b\\").as_bytes());
    assert!(replies(&mut t).contains("switched off"));
    assert!(!shown(&t, 0, 0));
    media::set_level(Level::Files);
    t.feed(format!("\x1b_Ga=T,t=f,f=24,s=1,v=1,i=1;{name}\x1b\\").as_bytes());
    media::set_level(Level::None);
    assert!(shown(&t, 0, 0), "{}", replies(&mut t));
    assert_eq!(pixel_at(&t, 0, 0), [255, 0, 0, 255]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_unicode_placeholder_picture_is_shown_where_the_program_prints_it() {
    let mut t = term();
    t.feed(format!("\x1b_Ga=T,U=1,f=24,s=1,v=1,i=5,c=2,r=1,q=2;{RED}\x1b\\").as_bytes());
    assert!(!shown(&t, 0, 0), "nothing is stamped at the cursor");
    t.feed("\x1b[38;5;5m\u{10EEEE}\u{305}\u{305}\u{10EEEE}\u{10EEEE}".as_bytes());
    assert!(shown(&t, 0, 0) && shown(&t, 1, 0), "two pieces");
    assert!(!shown(&t, 2, 0), "the third is outside a 2 cell wide picture");
    t.feed(b"\x1b_Ga=d,d=I,i=5\x1b\\");
    assert!(!shown(&t, 0, 0), "deleting the picture takes its pieces away");
    t.feed("\x1b[2;1H\x1b[38;5;5m\u{10EEEE}".as_bytes());
    assert!(!shown(&t, 0, 1), "and the id is gone");
}

#[test]
fn an_animation_plays_its_frames_on_time_and_stops_when_told() {
    let mut t = term();
    put(&mut t, "i=1", RED);
    t.feed(format!("\x1b_Ga=f,f=24,s=1,v=1,i=1,z=30,q=2;{GREEN}\x1b\\").as_bytes());
    t.feed(format!("\x1b_Ga=f,f=24,s=1,v=1,i=1,z=30,q=2;{BLUE}\x1b\\").as_bytes());
    assert_eq!(pixel_at(&t, 0, 0), [255, 0, 0, 255], "still the first frame");
    assert!(t.next_frame_due().is_none(), "not playing yet");
    t.feed(b"\x1b_Ga=a,i=1,s=3,v=1,q=2\x1b\\");
    let due = t.next_frame_due().expect("playing");
    assert!(t.advance_frames(due + Duration::from_millis(1)));
    assert_eq!(pixel_at(&t, 0, 0), [0, 255, 0, 255]);
    let due = t.next_frame_due().expect("still playing");
    assert!(t.advance_frames(due + Duration::from_millis(1)));
    assert_eq!(pixel_at(&t, 0, 0), [0, 0, 255, 255]);
    t.feed(b"\x1b_Ga=a,i=1,s=1,q=2\x1b\\");
    assert!(t.next_frame_due().is_none());
    assert!(!t.advance_frames(Instant::now() + Duration::from_secs(5)));
}

#[test]
fn frames_are_edited_composed_and_deleted() {
    let mut t = term();
    put(&mut t, "i=1", RED);
    t.feed(format!("\x1b_Ga=f,f=24,s=1,v=1,i=1,q=2;{GREEN}\x1b\\").as_bytes());
    t.feed(b"\x1b_Ga=a,i=1,c=2,q=2\x1b\\");
    assert_eq!(pixel_at(&t, 0, 0), [0, 255, 0, 255], "c= shows frame 2");
    t.feed(format!("\x1b_Ga=f,f=24,s=1,v=1,i=1,r=2,q=2;{BLUE}\x1b\\").as_bytes());
    assert_eq!(pixel_at(&t, 0, 0), [0, 0, 255, 255], "r=2 edits the frame being shown");
    t.feed(b"\x1b_Ga=c,i=1,r=1,c=2,C=1,q=2\x1b\\");
    assert_eq!(pixel_at(&t, 0, 0), [255, 0, 0, 255], "frame 1 composed onto frame 2");
    t.feed(b"\x1b_Ga=a,i=1,c=1,q=2\x1b\\\x1b_Ga=d,d=f,i=1\x1b\\");
    assert_eq!(pixel_at(&t, 0, 0), [255, 0, 0, 255], "the picture stays as its first frame");
    t.feed(b"\x1b_Ga=a,i=1,s=3,q=2\x1b\\");
    assert!(replies(&mut t).is_empty());
}

#[test]
fn bad_animation_commands_answer_with_an_error_and_change_nothing() {
    let mut t = term();
    t.feed(format!("\x1b_Ga=f,f=24,s=1,v=1,i=42;{GREEN}\x1b\\").as_bytes());
    assert!(replies(&mut t).contains("EINVAL"), "no picture with that id");
    put(&mut t, "i=1", RED);
    t.feed(b"\x1b_Ga=a,i=1,c=9\x1b\\");
    assert!(replies(&mut t).contains("EINVAL"), "frame 9 does not exist");
    t.feed(b"\x1b_Ga=c,i=1\x1b\\");
    assert!(replies(&mut t).contains("EINVAL"), "composing names no frames");
    for _ in 0..80 {
        t.feed(format!("\x1b_Ga=f,f=24,s=1,v=1,i=1,q=2;{GREEN}\x1b\\").as_bytes());
    }
    assert_eq!(pixel_at(&t, 0, 0), [255, 0, 0, 255], "the first frame is still shown");
}
