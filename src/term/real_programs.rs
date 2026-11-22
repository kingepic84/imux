//! Replays recordings of real programs (captured with `script` at 80x24) through the grid.

use super::Terminal;

const LESS: &[u8] = include_bytes!("../../tests/fixtures/less_end.bin");
const VIM: &[u8] = include_bytes!("../../tests/fixtures/vim_edit.bin");

const ALT_EXIT: &[u8] = b"\x1b[?1049l";

fn before_alt_exit(bytes: &[u8]) -> &[u8] {
    let pos = bytes.windows(ALT_EXIT.len()).rposition(|w| w == ALT_EXIT).expect("recording leaves the alt screen");
    &bytes[..pos]
}

fn screen(t: &Terminal) -> Vec<String> {
    t.grid.screen.iter().map(|r| r.text()).collect()
}

#[test]
fn less_shows_end_of_file_then_restores_main_screen() {
    let mut t = Terminal::new(80, 24, 100);
    t.feed(before_alt_exit(LESS));
    assert!(t.grid.is_alt());
    let s = screen(&t);
    assert_eq!(s[0], "line 78");
    assert_eq!(s[22], "line 100");
    assert_eq!(s[21], "line 99");
    t.feed(LESS);
    assert!(!t.grid.is_alt());
}

#[test]
fn vim_edit_session_screen() {
    let mut t = Terminal::new(80, 24, 100);
    t.feed(before_alt_exit(VIM));
    assert!(t.grid.is_alt());
    let s = screen(&t);
    assert_eq!(s[0], "hello world");
    assert_eq!(s[1], "second line");
    assert_eq!(s[2], "~");
    assert_eq!(s[22], "~");
}

#[test]
fn vim_leaves_cursor_and_modes_sane_after_exit() {
    let mut t = Terminal::new(80, 24, 100);
    t.feed(VIM);
    assert!(!t.grid.is_alt());
    assert!(t.grid.modes.cursor_visible);
    assert!(!t.grid.modes.bracketed_paste);
    assert!(!t.grid.modes.mouse_sgr);
}
