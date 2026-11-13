//! What the game tab sends to the game: X keysyms for the keys of egui,
//! X pointer buttons, and the place on the game's screen under the mouse.
//!
//! The keysyms are those of an unshifted US keyboard (the nested X server
//! has that layout): the game sees the key, not the character, so `W` walks
//! with any layout of the host (the tab uses the physical key).

use egui::{Key, PointerButton, Pos2, Rect, Vec2};

/// The X keysym of `key` (`X11/keysymdef.h`), `None` for keys the game
/// does not know.
pub fn keysym(key: Key) -> Option<u32> {
    use Key as K;
    let latin = |c: u8| Some(u32::from(c));
    match key {
        K::A => latin(b'a'),
        K::B => latin(b'b'),
        K::C => latin(b'c'),
        K::D => latin(b'd'),
        K::E => latin(b'e'),
        K::F => latin(b'f'),
        K::G => latin(b'g'),
        K::H => latin(b'h'),
        K::I => latin(b'i'),
        K::J => latin(b'j'),
        K::K => latin(b'k'),
        K::L => latin(b'l'),
        K::M => latin(b'm'),
        K::N => latin(b'n'),
        K::O => latin(b'o'),
        K::P => latin(b'p'),
        K::Q => latin(b'q'),
        K::R => latin(b'r'),
        K::S => latin(b's'),
        K::T => latin(b't'),
        K::U => latin(b'u'),
        K::V => latin(b'v'),
        K::W => latin(b'w'),
        K::X => latin(b'x'),
        K::Y => latin(b'y'),
        K::Z => latin(b'z'),
        K::Num0 => latin(b'0'),
        K::Num1 => latin(b'1'),
        K::Num2 => latin(b'2'),
        K::Num3 => latin(b'3'),
        K::Num4 => latin(b'4'),
        K::Num5 => latin(b'5'),
        K::Num6 => latin(b'6'),
        K::Num7 => latin(b'7'),
        K::Num8 => latin(b'8'),
        K::Num9 => latin(b'9'),
        K::Space => latin(b' '),
        K::Minus => latin(b'-'),
        K::Equals | K::Plus => latin(b'='),
        K::OpenBracket | K::OpenCurlyBracket => latin(b'['),
        K::CloseBracket | K::CloseCurlyBracket => latin(b']'),
        K::Backslash | K::Pipe => latin(b'\\'),
        K::Semicolon | K::Colon => latin(b';'),
        K::Quote => latin(b'\''),
        K::Comma => latin(b','),
        K::Period => latin(b'.'),
        K::Slash | K::Questionmark => latin(b'/'),
        K::Backtick => latin(b'`'),
        K::Exclamationmark => latin(b'1'),
        K::Escape => Some(0xff1b),
        K::Tab => Some(0xff09),
        K::Backspace => Some(0xff08),
        K::Enter => Some(0xff0d),
        K::Insert => Some(0xff63),
        K::Delete => Some(0xffff),
        K::Home => Some(0xff50),
        K::End => Some(0xff57),
        K::PageUp => Some(0xff55),
        K::PageDown => Some(0xff56),
        K::ArrowLeft => Some(0xff51),
        K::ArrowUp => Some(0xff52),
        K::ArrowRight => Some(0xff53),
        K::ArrowDown => Some(0xff54),
        K::ShiftLeft => Some(0xffe1),
        K::ShiftRight => Some(0xffe2),
        K::ControlLeft => Some(0xffe3),
        K::ControlRight => Some(0xffe4),
        K::AltLeft => Some(0xffe9),
        K::AltRight => Some(0xffea),
        K::CapsLock => Some(0xffe5),
        K::Pause => Some(0xff13),
        K::PrintScreen => Some(0xff61),
        K::Menu => Some(0xff67),
        K::F1 => Some(0xffbe),
        K::F2 => Some(0xffbf),
        K::F3 => Some(0xffc0),
        K::F4 => Some(0xffc1),
        K::F5 => Some(0xffc2),
        K::F6 => Some(0xffc3),
        K::F7 => Some(0xffc4),
        K::F8 => Some(0xffc5),
        K::F9 => Some(0xffc6),
        K::F10 => Some(0xffc7),
        K::F11 => Some(0xffc8),
        K::F12 => Some(0xffc9),
        _ => None,
    }
}

/// The X button of a mouse button: 1 left, 2 middle, 3 right, 8 and 9 the
/// side buttons.
pub fn x_button(button: PointerButton) -> u8 {
    match button {
        PointerButton::Primary => 1,
        PointerButton::Middle => 2,
        PointerButton::Secondary => 3,
        PointerButton::Extra1 => 8,
        PointerButton::Extra2 => 9,
    }
}

/// The X buttons a wheel turn presses: 4 up, 5 down, 6 left, 7 right, one
/// click per notch (`delta` in lines: egui gives points for touchpads,
/// divided by `per_line`).
pub fn wheel_buttons(delta: Vec2, per_line: f32) -> Vec<u8> {
    let notches = |v: f32| (v / per_line.max(1e-3)).round() as i32;
    let mut out = Vec::new();
    let (y, x) = (notches(delta.y), notches(delta.x));
    out.extend(std::iter::repeat_n(
        if y > 0 { 4 } else { 5 },
        y.unsigned_abs() as usize,
    ));
    out.extend(std::iter::repeat_n(
        if x > 0 { 6 } else { 7 },
        x.unsigned_abs() as usize,
    ));
    out
}

/// Where the game's screen of `size` pixels is drawn inside `available`:
/// as large as fits, with its proportions, centred. Pixel-exact (scale 1)
/// when the tab has room for it and is not much larger.
pub fn fit(available: Rect, size: [u32; 2], pixels_per_point: f32) -> Rect {
    let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
    let ppp = pixels_per_point.max(1e-3);
    let mut scale = (available.width() / w).min(available.height() / h);
    // One game pixel per screen pixel, when that is within 10 %.
    let exact = 1.0 / ppp;
    if (scale - exact).abs() < 0.1 * exact {
        scale = exact;
    }
    Rect::from_center_size(available.center(), egui::vec2(w * scale, h * scale))
}

/// The pixel of the game's screen under `pos` when the screen is drawn in
/// `image`; `None` outside it.
#[cfg_attr(windows, allow(dead_code))]
pub fn to_game(pos: Pos2, image: Rect, size: [u32; 2]) -> Option<[i32; 2]> {
    if !image.contains(pos) || image.width() <= 0.0 || image.height() <= 0.0 {
        return None;
    }
    let u = (pos.x - image.min.x) / image.width();
    let v = (pos.y - image.min.y) / image.height();
    let x = ((u * size[0] as f32) as i32).clamp(0, size[0] as i32 - 1);
    let y = ((v * size[1] as f32) as i32).clamp(0, size[1] as i32 - 1);
    Some([x, y])
}

/// Mouse motion in points made into whole game pixels: `rest` keeps the
/// fractions between frames, so slow motion is not lost.
pub fn motion(delta: Vec2, scale: f32, rest: &mut Vec2) -> [i32; 2] {
    let total = delta * scale + *rest;
    let whole = egui::vec2(total.x.trunc(), total.y.trunc());
    *rest = total - whole;
    [whole.x as i32, whole.y as i32]
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    #[test]
    fn keys_are_unshifted_keysyms() {
        assert_eq!(keysym(Key::W), Some(0x77));
        assert_eq!(keysym(Key::Num1), Some(0x31));
        assert_eq!(keysym(Key::Space), Some(0x20));
        assert_eq!(keysym(Key::Escape), Some(0xff1b));
        assert_eq!(keysym(Key::Enter), Some(0xff0d));
        assert_eq!(keysym(Key::ShiftLeft), Some(0xffe1));
        assert_eq!(keysym(Key::F12), Some(0xffc9));
        // Characters of a shifted key are the key itself.
        assert_eq!(keysym(Key::Colon), keysym(Key::Semicolon));
        assert_eq!(keysym(Key::Plus), keysym(Key::Equals));
        assert_eq!(keysym(Key::BrowserBack), None);
        // Every letter and digit is mapped, to distinct keysyms.
        let mut seen = std::collections::HashSet::new();
        for key in Key::ALL {
            let name = key.name();
            if name.len() == 1 && name.chars().all(|c| c.is_ascii_alphanumeric()) {
                assert!(seen.insert(keysym(*key).expect(name)), "{name}");
            }
        }
        assert_eq!(seen.len(), 36);
    }

    #[test]
    fn buttons_and_wheel() {
        assert_eq!(x_button(PointerButton::Primary), 1);
        assert_eq!(x_button(PointerButton::Secondary), 3);
        assert_eq!(x_button(PointerButton::Middle), 2);
        assert_eq!(wheel_buttons(vec2(0.0, 2.0), 1.0), vec![4, 4]);
        assert_eq!(wheel_buttons(vec2(0.0, -40.0), 40.0), vec![5]);
        assert_eq!(wheel_buttons(vec2(-1.0, 0.0), 1.0), vec![7]);
        assert!(wheel_buttons(vec2(0.0, 0.2), 1.0).is_empty());
    }

    #[test]
    fn the_screen_keeps_its_proportions() {
        // A wide tab: bars left and right.
        let tab = Rect::from_min_size(pos2(0.0, 0.0), vec2(2000.0, 720.0));
        let image = fit(tab, [1280, 720], 1.0);
        assert_eq!(
            image,
            Rect::from_min_size(pos2(360.0, 0.0), vec2(1280.0, 720.0))
        );
        // A small tab: scaled down, centred.
        let tab = Rect::from_min_size(pos2(10.0, 20.0), vec2(640.0, 600.0));
        let image = fit(tab, [1280, 720], 1.0);
        assert_eq!(image.size(), vec2(640.0, 360.0));
        assert_eq!(image.center(), tab.center());
        // Nearly the game's size: one game pixel per screen pixel.
        let tab = Rect::from_min_size(pos2(0.0, 0.0), vec2(1300.0, 740.0));
        assert_eq!(fit(tab, [1280, 720], 1.0).size(), vec2(1280.0, 720.0));
        // At 2 pixels per point 640×360 points are the game's pixels.
        let tab = Rect::from_min_size(pos2(0.0, 0.0), vec2(650.0, 370.0));
        assert_eq!(fit(tab, [1280, 720], 2.0).size(), vec2(640.0, 360.0));
    }

    #[test]
    fn positions_map_to_game_pixels() {
        let image = Rect::from_min_size(pos2(100.0, 50.0), vec2(640.0, 360.0));
        let size = [1280, 720];
        assert_eq!(to_game(pos2(100.0, 50.0), image, size), Some([0, 0]));
        assert_eq!(to_game(pos2(420.0, 230.0), image, size), Some([640, 360]));
        assert_eq!(to_game(pos2(740.0, 410.0), image, size), Some([1279, 719]));
        assert_eq!(to_game(pos2(99.0, 60.0), image, size), None);
        assert_eq!(to_game(pos2(300.0, 411.0), image, size), None);
    }

    #[test]
    fn slow_motion_adds_up() {
        let mut rest = Vec2::ZERO;
        let mut sum = [0, 0];
        for _ in 0..10 {
            let [x, y] = motion(vec2(0.5, -0.25), 1.0, &mut rest);
            sum[0] += x;
            sum[1] += y;
        }
        assert_eq!(sum, [5, -2]);
        assert_eq!(motion(vec2(2.0, 1.0), 2.0, &mut Vec2::default()), [4, 2]);
    }
}
