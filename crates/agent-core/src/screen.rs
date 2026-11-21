//! Screen control: the AI sees this
//! Mac's screen and works its mouse and keyboard — `screen_screenshot` and `screen_action` —
//! **only while the person has turned on "Let Wysebee see and control this Mac's screen"**
//! (`agent.rs`), and macOS has given the app Screen Recording and Accessibility. Made for
//! trying an app on the Mac from Wysebee: Claude, through Wysebee Assistant, takes a screenshot, clicks,
//! types and looks again.
//!
//! [`Screen`] is the seam: the shell installs the real one (the primary display, captured, and
//! the mouse and keyboard, with a ring shown where the pointer acts); **the tests a fake**.
//! Everything else is here and pure: the picture the AI gets ([`picture`]: scaled to at most
//! [`MAX_SIDE`] pixels on its long side and [`MAX_PIXELS`] in all, the pointer drawn in, as a
//! JPEG) and an action read from a call ([`parse`]), its coordinates **in that picture's
//! pixels**, turned into the screen's.
//!
//! Nothing here checks what is typed or clicked: the person's switch is the safeguard. Typed
//! text is never logged.

use base64::Engine;
use serde_json::{json, Value};

/// The picture's long side, at most (what a model reads a screen at well).
pub const MAX_SIDE: u32 = 1568;
/// The picture's pixels, at most.
pub const MAX_PIXELS: f64 = 1_150_000.0;
/// Text typed in one action, at most.
pub const MAX_TEXT: usize = 2_000;
/// Keys pressed together, at most.
pub const MAX_KEYS: usize = 5;
/// Scroll notches in one action, at most.
pub const MAX_SCROLL: i32 = 20;
const JPEG_QUALITY: u8 = 75;

/// One captured screen: its pixels (RGBA, row by row) and where the pointer is on it.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub cursor: Option<(i32, i32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// What the pointer or the keyboard does, in the screen's own pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Move { x: i32, y: i32 },
    Click { x: i32, y: i32, button: Button, count: u8 },
    Drag { x: i32, y: i32, to_x: i32, to_y: i32 },
    /// Notches: `dy` down when positive, `dx` right when positive.
    Scroll { x: i32, y: i32, dx: i32, dy: i32 },
    Type { text: String },
    /// Pressed together, in order, then let go in reverse (`ctrl+shift+t`).
    Key { keys: Vec<String> },
}

impl Action {
    /// What was done, for the AI to read: never the typed text.
    pub fn said(&self) -> String {
        match self {
            Action::Move { x, y } => format!("Moved the pointer to {x}, {y} on the screen."),
            Action::Click { x, y, button, count } => format!("{} at {x}, {y} on the screen.", match (button, count) {
                (Button::Left, 2) => "Double-clicked",
                (Button::Left, _) => "Clicked",
                (Button::Right, _) => "Right-clicked",
                (Button::Middle, _) => "Middle-clicked",
            }),
            Action::Drag { x, y, to_x, to_y } => format!("Dragged from {x}, {y} to {to_x}, {to_y} on the screen."),
            Action::Scroll { x, y, dx, dy } => format!("Scrolled {dx} right and {dy} down at {x}, {y} on the screen."),
            Action::Type { text } => format!("Typed {} characters.", text.chars().count()),
            Action::Key { keys } => format!("Pressed {}.", keys.join("+")),
        }
    }
}

/// The display, the mouse and the keyboard. Blocking: called off the async threads.
pub trait Screen: Send + Sync {
    /// The primary display, now.
    fn capture(&self) -> Result<Frame, String>;
    /// Its size in pixels.
    fn size(&self) -> Result<(u32, u32), String>;
    /// Do it (in the screen's pixels).
    fn act(&self, action: &Action) -> Result<(), String>;
}

/// The keys an action may press: what a keyboard has, by name.
pub const KEYS: &[&str] = &[
    "cmd", "ctrl", "alt", "shift", "enter", "tab", "escape", "backspace", "delete", "space", "up", "down", "left", "right", "home", "end", "pageup", "pagedown", "capslock", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
];

/// A key's name as [`KEYS`] has it (`Ctrl` → `ctrl`, `command` → `cmd`, `option` → `alt`,
/// `return` → `enter`, `esc` → `escape`),
/// or one letter or digit; None when it is no key here.
fn key(name: &str) -> Option<String> {
    let lower = name.trim().to_lowercase();
    let named = match lower.as_str() {
        "control" => "ctrl",
        "return" => "enter",
        "esc" => "escape",
        "del" => "delete",
        "command" | "meta" | "super" | "win" | "windows" => "cmd",
        "option" | "opt" => "alt",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        other => other,
    };
    if KEYS.contains(&named) {
        return Some(named.to_string());
    }
    let mut chars = named.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() || "`-=[]\\;',./".contains(c) => Some(c.to_string()),
        _ => None,
    }
}

/// The picture's size for a screen of `width` × `height`: never larger than the screen.
pub fn fit(width: u32, height: u32) -> (u32, u32) {
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let scale = (MAX_SIDE as f64 / w.max(h)).min((MAX_PIXELS / (w * h)).sqrt()).min(1.0);
    (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1))
}

/// A frame as the AI gets it: `{image (JPEG, base64), mime, width, height (the picture's),
/// screen_width, screen_height, cursor: [x, y] in the picture or null}`, the pointer drawn in.
pub fn picture(frame: &Frame) -> Result<Value, String> {
    let full = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba.clone()).ok_or("The screen's picture could not be read.")?;
    let (w, h) = fit(frame.width, frame.height);
    let mut small = image::DynamicImage::ImageRgba8(full).resize_exact(w, h, image::imageops::FilterType::Triangle).to_rgb8();
    let cursor = frame.cursor.map(|(x, y)| (x as f64 * w as f64 / frame.width as f64, y as f64 * h as f64 / frame.height as f64)).map(|(x, y)| (x.round() as i32, y.round() as i32));
    if let Some((x, y)) = cursor {
        mark(&mut small, x, y);
    }
    let mut jpeg = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY).encode_image(&small).map_err(|e| format!("The screen's picture could not be made: {e}"))?;
    Ok(json!({
        "image": base64::engine::general_purpose::STANDARD.encode(&jpeg),
        "mime": "image/jpeg",
        "width": w,
        "height": h,
        "screen_width": frame.width,
        "screen_height": frame.height,
        "cursor": cursor.map(|(x, y)| json!([x, y])),
    }))
}

/// The pointer drawn on the picture: a red ring with a dot, so the AI sees where it is.
fn mark(img: &mut image::RgbImage, cx: i32, cy: i32) {
    let red = image::Rgb([230, 30, 60]);
    for dy in -10i32..=10 {
        for dx in -10i32..=10 {
            let d2 = dx * dx + dy * dy;
            let (x, y) = (cx + dx, cy + dy);
            if (d2 <= 4 || (64..=100).contains(&d2)) && x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                img.put_pixel(x as u32, y as u32, red);
            }
        }
    }
}

/// An action as a `screen_action` call says it, its coordinates in the picture of a screen of
/// `screen` size (as [`fit`] makes it) turned into the screen's own.
pub fn parse(args: &Value, screen: (u32, u32)) -> Result<Action, String> {
    let (pw, ph) = fit(screen.0, screen.1);
    let at = |kx: &str, ky: &str| -> Result<(i32, i32), String> {
        let n = |k: &str, most: u32| -> Result<i32, String> {
            let v = args.get(k).and_then(Value::as_f64).ok_or(format!("Give '{k}', a number: where on the screenshot."))?;
            if !(0.0..most as f64).contains(&v) {
                return Err(format!("'{k}' is outside the screenshot ({pw} × {ph}): take a new screenshot."));
            }
            Ok(v.round() as i32)
        };
        let (x, y) = (n(kx, pw)?, n(ky, ph)?);
        let to = |v: i32, p: u32, s: u32| ((v as f64 + 0.5) * s as f64 / p as f64).floor().min(s as f64 - 1.0) as i32;
        Ok((to(x, pw, screen.0), to(y, ph, screen.1)))
    };
    let action = args.get("action").and_then(Value::as_str).unwrap_or("");
    let click = |button, count| -> Result<Action, String> {
        let (x, y) = at("x", "y")?;
        Ok(Action::Click { x, y, button, count })
    };
    match action {
        "move" => at("x", "y").map(|(x, y)| Action::Move { x, y }),
        "click" => click(Button::Left, 1),
        "double_click" => click(Button::Left, 2),
        "right_click" => click(Button::Right, 1),
        "middle_click" => click(Button::Middle, 1),
        "drag" => {
            let (x, y) = at("x", "y")?;
            let (to_x, to_y) = at("to_x", "to_y")?;
            Ok(Action::Drag { x, y, to_x, to_y })
        }
        "scroll" => {
            let (x, y) = at("x", "y")?;
            let amount = args.get("amount").and_then(Value::as_i64).unwrap_or(3);
            if !(1..=MAX_SCROLL as i64).contains(&amount) {
                return Err(format!("'amount' is 1 to {MAX_SCROLL} notches."));
            }
            let n = amount as i32;
            let (dx, dy) = match args.get("direction").and_then(Value::as_str).unwrap_or("down") {
                "down" => (0, n),
                "up" => (0, -n),
                "right" => (n, 0),
                "left" => (-n, 0),
                _ => return Err("'direction' is up, down, left or right.".into()),
            };
            Ok(Action::Scroll { x, y, dx, dy })
        }
        "type" => {
            let text = args.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() || text.chars().count() > MAX_TEXT {
                return Err(format!("Give 'text' to type: 1 to {MAX_TEXT} characters."));
            }
            Ok(Action::Type { text: text.to_string() })
        }
        "key" => {
            let given = args.get("keys").and_then(Value::as_str).unwrap_or("");
            let names: Vec<&str> = given.split('+').filter(|k| !k.trim().is_empty()).collect();
            if names.is_empty() || names.len() > MAX_KEYS {
                return Err(format!("Give 'keys', like enter or cmd+s: 1 to {MAX_KEYS} keys joined by +."));
            }
            let keys = names.iter().map(|k| key(k).ok_or(format!("'{}' is not a key here: a letter, a digit, or one of {}.", k.trim(), KEYS.join(", ")))).collect::<Result<Vec<_>, _>>()?;
            Ok(Action::Key { keys })
        }
        other => Err(format!("'{other}' is not an action: move, click, double_click, right_click, middle_click, drag, scroll, type or key.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, cursor: Option<(i32, i32)>) -> Frame {
        Frame { width, height, rgba: vec![255; (width * height * 4) as usize], cursor }
    }

    #[test]
    fn a_picture_is_never_larger_than_a_model_reads_well_nor_the_screen() {
        assert_eq!(fit(1920, 1080), (1430, 804));
        assert_eq!(fit(3840, 2160), (1430, 804));
        assert_eq!(fit(1024, 768), (1024, 768), "a small screen is not enlarged");
        assert_eq!(fit(1568, 400), (1568, 400));
        assert_eq!(fit(4000, 400), (1568, 157));
        assert_eq!(fit(0, 0), (1, 1));
    }

    #[test]
    fn a_picture_is_a_jpeg_with_the_pointer_drawn_in() {
        let shot = picture(&frame(1920, 1080, Some((960, 540)))).unwrap();
        assert_eq!((shot["width"].as_u64(), shot["height"].as_u64(), shot["mime"].as_str()), (Some(1430), Some(804), Some("image/jpeg")));
        assert_eq!((shot["screen_width"].as_u64(), shot["screen_height"].as_u64()), (Some(1920), Some(1080)));
        assert_eq!(shot["cursor"], json!([715, 402]));
        let jpeg = base64::engine::general_purpose::STANDARD.decode(shot["image"].as_str().unwrap()).unwrap();
        let back = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!(back.dimensions(), (1430, 804));
        // The ring is red; the screen around it white.
        let ring = back.get_pixel(715 + 9, 402);
        assert!(ring[0] > 150 && ring[1] < 120, "{ring:?}");
        assert!(back.get_pixel(10, 10)[1] > 240);
        // No pointer, or one at an edge: drawn as far as it fits.
        assert_eq!(picture(&frame(800, 600, None)).unwrap()["cursor"], Value::Null);
        assert_eq!(picture(&frame(800, 600, Some((0, 0)))).unwrap()["cursor"], json!([0, 0]));
        assert_eq!(picture(&frame(800, 600, Some((799, 599)))).unwrap()["cursor"], json!([799, 599]));
        let short = Frame { width: 10, height: 10, rgba: vec![0; 3], cursor: None };
        assert_eq!(picture(&short).unwrap_err(), "The screen's picture could not be read.");
    }

    #[test]
    fn an_actions_coordinates_are_the_pictures_turned_into_the_screens() {
        let screen = (1920, 1080);
        assert_eq!(parse(&json!({"action": "click", "x": 715, "y": 402}), screen).unwrap(), Action::Click { x: 960, y: 540, button: Button::Left, count: 1 });
        assert_eq!(parse(&json!({"action": "double_click", "x": 0, "y": 0}), screen).unwrap(), Action::Click { x: 0, y: 0, button: Button::Left, count: 2 });
        assert_eq!(parse(&json!({"action": "right_click", "x": 1429.4, "y": 803}), screen).unwrap(), Action::Click { x: 1919, y: 1079, button: Button::Right, count: 1 });
        assert_eq!(parse(&json!({"action": "middle_click", "x": 10, "y": 10}), screen).unwrap(), Action::Click { x: 14, y: 14, button: Button::Middle, count: 1 });
        assert_eq!(parse(&json!({"action": "move", "x": 100, "y": 50}), (800, 600)).unwrap(), Action::Move { x: 100, y: 50 });
        assert_eq!(parse(&json!({"action": "drag", "x": 1, "y": 2, "to_x": 3, "to_y": 4}), (800, 600)).unwrap(), Action::Drag { x: 1, y: 2, to_x: 3, to_y: 4 });
        assert_eq!(parse(&json!({"action": "scroll", "x": 1, "y": 2}), (800, 600)).unwrap(), Action::Scroll { x: 1, y: 2, dx: 0, dy: 3 });
        for (direction, dx, dy) in [("up", 0, -5), ("down", 0, 5), ("left", -5, 0), ("right", 5, 0)] {
            assert_eq!(parse(&json!({"action": "scroll", "x": 1, "y": 2, "direction": direction, "amount": 5}), (800, 600)).unwrap(), Action::Scroll { x: 1, y: 2, dx, dy });
        }
        let refuse = |v: Value| parse(&v, (800, 600)).unwrap_err();
        assert_eq!(refuse(json!({"action": "click", "y": 1})), "Give 'x', a number: where on the screenshot.");
        assert!(refuse(json!({"action": "click", "x": 800, "y": 1})).contains("outside the screenshot (800 × 600)"));
        assert!(refuse(json!({"action": "click", "x": -1, "y": 1})).contains("outside"));
        assert!(refuse(json!({"action": "drag", "x": 1, "y": 1, "to_x": 1})).contains("'to_y'"));
        assert!(refuse(json!({"action": "scroll", "x": 1, "y": 1, "amount": 21})).contains("1 to 20"));
        assert!(refuse(json!({"action": "scroll", "x": 1, "y": 1, "direction": "back"})).contains("up, down, left or right"));
        assert!(refuse(json!({"action": "fly"})).starts_with("'fly' is not an action"));
        assert!(refuse(json!({})).starts_with("'' is not an action"));
    }

    #[test]
    fn typed_text_and_keys_are_checked() {
        let s = (800, 600);
        assert_eq!(parse(&json!({"action": "type", "text": "héllo"}), s).unwrap(), Action::Type { text: "héllo".into() });
        assert!(parse(&json!({"action": "type", "text": ""}), s).unwrap_err().contains("1 to 2000"));
        assert!(parse(&json!({"action": "type", "text": "x".repeat(2001)}), s).unwrap_err().contains("1 to 2000"));
        assert_eq!(parse(&json!({"action": "key", "keys": "Control+Shift+t"}), s).unwrap(), Action::Key { keys: vec!["ctrl".into(), "shift".into(), "t".into()] });
        assert_eq!(parse(&json!({"action": "key", "keys": "Return"}), s).unwrap(), Action::Key { keys: vec!["enter".into()] });
        assert_eq!(parse(&json!({"action": "key", "keys": "esc+del+windows+pgup+pgdn+F5"}), s).unwrap_err(), "Give 'keys', like enter or cmd+s: 1 to 5 keys joined by +.");
        assert_eq!(parse(&json!({"action": "key", "keys": "esc+del+command+pgup+F5"}), s).unwrap(), Action::Key { keys: ["escape", "delete", "cmd", "pageup", "f5"].map(String::from).to_vec() });
        assert_eq!(parse(&json!({"action": "key", "keys": "pgdn+option+/"}), s).unwrap(), Action::Key { keys: ["pagedown", "alt", "/"].map(String::from).to_vec() });
        assert!(parse(&json!({"action": "key", "keys": ""}), s).unwrap_err().starts_with("Give 'keys'"));
        assert!(parse(&json!({"action": "key", "keys": "ctrl+hyper"}), s).unwrap_err().starts_with("'hyper' is not a key here"));
        assert!(parse(&json!({"action": "key", "keys": "é"}), s).unwrap_err().starts_with("'é' is not a key here"));
    }

    #[test]
    fn what_was_done_is_said_without_the_typed_text() {
        let says = |a: Action| a.said();
        assert_eq!(says(Action::Move { x: 1, y: 2 }), "Moved the pointer to 1, 2 on the screen.");
        assert_eq!(says(Action::Click { x: 1, y: 2, button: Button::Left, count: 1 }), "Clicked at 1, 2 on the screen.");
        assert_eq!(says(Action::Click { x: 1, y: 2, button: Button::Left, count: 2 }), "Double-clicked at 1, 2 on the screen.");
        assert_eq!(says(Action::Click { x: 1, y: 2, button: Button::Right, count: 1 }), "Right-clicked at 1, 2 on the screen.");
        assert_eq!(says(Action::Click { x: 1, y: 2, button: Button::Middle, count: 1 }), "Middle-clicked at 1, 2 on the screen.");
        assert_eq!(says(Action::Drag { x: 1, y: 2, to_x: 3, to_y: 4 }), "Dragged from 1, 2 to 3, 4 on the screen.");
        assert_eq!(says(Action::Scroll { x: 1, y: 2, dx: 0, dy: -3 }), "Scrolled 0 right and -3 down at 1, 2 on the screen.");
        assert_eq!(says(Action::Type { text: "secret".into() }), "Typed 6 characters.");
        assert_eq!(says(Action::Key { keys: vec!["ctrl".into(), "s".into()] }), "Pressed ctrl+s.");
    }
}
