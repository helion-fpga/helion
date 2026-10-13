//! In-process egui event fuzzer for the Helion IDE (test-only, never shipped).
//!
//! Driving mechanism: the REAL `HelionIde` frame path. The fuzzer calls
//! `HelionIde::update_inner` - the exact body of `eframe::App::update`,
//! factored out so headless drivers share it with zero duplication.
//! feeds synthetic `egui::Event`s into `egui::Context::run`, which runs the
//! full pass including tessellation, so paint-path panics are caught too.
//! No X server, no window, no `xdotool`: everything happens in-process.
//!
//! Determinism: seeded splitmix64 RNG plus a synthetic `RawInput.time` clock
//! (1/60 s per frame). Every run appends a line-based event log carrying the
//! seed and the full input sequence; `HELION_FUZZ_REPLAY=<log>` re-feeds the
//! identical sequence. Wall-clock leaks (toast expiry via `Instant`, engine
//! thread scheduling) can shift WHICH frame observes an async job result, but
//! the input sequence itself is exact.
//!
//! Env controls:
//!   HELION_FUZZ_RUN=1        actually run the fuzzer (without it the test is
//!                            a no-op so `cargo test` stays fast)
//!   HELION_FUZZ_SEED         u64 seed (default 0x9E3779B97F4A7C15)
//!   HELION_FUZZ_SEEDS        extra seeds, e.g. "1,2,3" or "7-9"
//!   HELION_FUZZ_FRAMES       frames per seed (default 20000)
//!   HELION_FUZZ_DEADLINE_SECS stop starting new seeds after N s (default 0 = none)
//!   HELION_FUZZ_LOGDIR       run-log directory (default <cwd>/fuzz_runs)
//!   HELION_FUZZ_REPLAY       replay an event log instead of fuzzing
//!   HELION_FUZZ_DESIGN       design opened during warmup
//!                            (default examples/blinky.sv, empty = none)
//!   HELION_FUZZ_WARMUP       1 = open design + run Implement before fuzzing
//!                            (default 1; gives the timing/device views data)
//!   HELION_FUZZ_QUIET        1 = less stdout
//!
//! Crash replays land in `crates/helion-gui/fuzz_crashes/` and are wired into
//! regression tests at the bottom of this file via `include_str!`.

use super::*;
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Seeded RNG (splitmix64; no new dependencies)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % (n as u64)) as usize
    }
    fn below_u64(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
    fn range_f(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * ((self.next_u64() >> 11) as f32 / ((1u64 << 53) as f32))
    }
    fn chance(&mut self, pct: u32) -> bool {
        self.below_u64(100) < pct as u64
    }
    fn pick<'a, T>(&mut self, s: &'a [T]) -> &'a T {
        &s[self.below(s.len())]
    }
}

// ---------------------------------------------------------------------------
// Serializable fuzz events
// ---------------------------------------------------------------------------

/// Button index: 0 = primary, 1 = secondary, 2 = middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Btn {
    Primary,
    Secondary,
    Middle,
}
impl Btn {
    fn idx(self) -> u8 {
        match self {
            Btn::Primary => 0,
            Btn::Secondary => 1,
            Btn::Middle => 2,
        }
    }
    fn from_idx(i: u8) -> Self {
        match i {
            1 => Btn::Secondary,
            2 => Btn::Middle,
            _ => Btn::Primary,
        }
    }
    fn egui(self) -> egui::PointerButton {
        match self {
            Btn::Primary => egui::PointerButton::Primary,
            Btn::Secondary => egui::PointerButton::Secondary,
            Btn::Middle => egui::PointerButton::Middle,
        }
    }
}

/// Modifier bitmask: 1 = shift, 2 = ctrl, 4 = alt, 8 = command.
fn mods_from_bits(bits: u8) -> egui::Modifiers {
    egui::Modifiers {
        alt: bits & 4 != 0,
        ctrl: bits & 2 != 0,
        shift: bits & 1 != 0,
        mac_cmd: false,
        command: bits & 8 != 0,
    }
}

#[derive(Clone, Debug)]
enum Fev {
    Move { x: f32, y: f32 },
    Down { x: f32, y: f32, btn: Btn, mods: u8 },
    Up { x: f32, y: f32, btn: Btn, mods: u8 },
    Wheel { x: f32, y: f32, dx: f32, dy: f32, unit: u8 },
    Key { key: &'static str, pressed: bool, mods: u8 },
    Text(char),
    Paste(String),
    Copy,
    Cut,
    Resize { w: f32, h: f32 },
    Zoom { f: f32 },
}

fn key_by_name(name: &str) -> egui::Key {
    match name {
        "Enter" => egui::Key::Enter,
        "Escape" => egui::Key::Escape,
        "Tab" => egui::Key::Tab,
        "Backspace" => egui::Key::Backspace,
        "Delete" => egui::Key::Delete,
        "Space" => egui::Key::Space,
        "ArrowUp" => egui::Key::ArrowUp,
        "ArrowDown" => egui::Key::ArrowDown,
        "ArrowLeft" => egui::Key::ArrowLeft,
        "ArrowRight" => egui::Key::ArrowRight,
        "Home" => egui::Key::Home,
        "End" => egui::Key::End,
        "PageUp" => egui::Key::PageUp,
        "PageDown" => egui::Key::PageDown,
        "Backtick" => egui::Key::Backtick,
        "0" => egui::Key::Num0,
        "1" => egui::Key::Num1,
        "2" => egui::Key::Num2,
        "3" => egui::Key::Num3,
        "4" => egui::Key::Num4,
        "5" => egui::Key::Num5,
        "6" => egui::Key::Num6,
        "7" => egui::Key::Num7,
        "8" => egui::Key::Num8,
        "9" => egui::Key::Num9,
        "F1" => egui::Key::F1,
        "F2" => egui::Key::F2,
        "F3" => egui::Key::F3,
        "F4" => egui::Key::F4,
        "F5" => egui::Key::F5,
        "F6" => egui::Key::F6,
        "F7" => egui::Key::F7,
        "F8" => egui::Key::F8,
        "F9" => egui::Key::F9,
        "F10" => egui::Key::F10,
        "F11" => egui::Key::F11,
        "F12" => egui::Key::F12,
        s if s.len() == 1 => {
            let c = s.chars().next().unwrap().to_ascii_uppercase();
            match c {
                'A' => egui::Key::A,
                'B' => egui::Key::B,
                'C' => egui::Key::C,
                'D' => egui::Key::D,
                'E' => egui::Key::E,
                'F' => egui::Key::F,
                'G' => egui::Key::G,
                'H' => egui::Key::H,
                'I' => egui::Key::I,
                'J' => egui::Key::J,
                'K' => egui::Key::K,
                'L' => egui::Key::L,
                'M' => egui::Key::M,
                'N' => egui::Key::N,
                'O' => egui::Key::O,
                'P' => egui::Key::P,
                'Q' => egui::Key::Q,
                'R' => egui::Key::R,
                'S' => egui::Key::S,
                'T' => egui::Key::T,
                'U' => egui::Key::U,
                'V' => egui::Key::V,
                'W' => egui::Key::W,
                'X' => egui::Key::X,
                'Y' => egui::Key::Y,
                'Z' => egui::Key::Z,
                _ => egui::Key::Space,
            }
        }
        _ => egui::Key::Space,
    }
}

impl Fev {
    fn to_egui(&self) -> Vec<egui::Event> {
        match self {
            Fev::Move { x, y } => vec![egui::Event::PointerMoved(egui::pos2(*x, *y))],
            Fev::Down { x, y, btn, mods } => vec![
                egui::Event::PointerMoved(egui::pos2(*x, *y)),
                egui::Event::PointerButton {
                    pos: egui::pos2(*x, *y),
                    button: btn.egui(),
                    pressed: true,
                    modifiers: mods_from_bits(*mods),
                },
            ],
            Fev::Up { x, y, btn, mods } => vec![
                egui::Event::PointerMoved(egui::pos2(*x, *y)),
                egui::Event::PointerButton {
                    pos: egui::pos2(*x, *y),
                    button: btn.egui(),
                    pressed: false,
                    modifiers: mods_from_bits(*mods),
                },
            ],
            Fev::Wheel { x, y, dx, dy, unit } => vec![
                egui::Event::PointerMoved(egui::pos2(*x, *y)),
                egui::Event::MouseWheel {
                    unit: match unit {
                        1 => egui::MouseWheelUnit::Line,
                        2 => egui::MouseWheelUnit::Page,
                        _ => egui::MouseWheelUnit::Point,
                    },
                    delta: egui::vec2(*dx, *dy),
                    modifiers: egui::Modifiers::default(),
                },
            ],
            Fev::Key { key, pressed, mods } => vec![egui::Event::Key {
                key: key_by_name(key),
                physical_key: None,
                pressed: *pressed,
                repeat: false,
                modifiers: mods_from_bits(*mods),
            }],
            Fev::Text(c) => vec![egui::Event::Text(c.to_string())],
            Fev::Paste(s) => vec![egui::Event::Paste(s.clone())],
            Fev::Copy => vec![egui::Event::Copy],
            Fev::Cut => vec![egui::Event::Cut],
            Fev::Resize { .. } => vec![],
            Fev::Zoom { f } => vec![egui::Event::Zoom(*f)],
        }
    }

    /// One line in the replay log (no newlines possible).
    fn serialize(&self) -> String {
        match self {
            Fev::Move { x, y } => format!("EV move {x:.2} {y:.2}"),
            Fev::Down { x, y, btn, mods } => {
                format!("EV down {x:.2} {y:.2} {} {mods}", btn.idx())
            }
            Fev::Up { x, y, btn, mods } => format!("EV up {x:.2} {y:.2} {} {mods}", btn.idx()),
            Fev::Wheel { x, y, dx, dy, unit } => {
                format!("EV wheel {x:.2} {y:.2} {dx:.2} {dy:.2} {unit}")
            }
            Fev::Key { key, pressed, mods } => {
                format!("EV key {key} {} {mods}", *pressed as u8)
            }
            Fev::Text(c) => format!("EV text {}", *c as u32),
            Fev::Paste(s) => {
                let cps: Vec<String> = s.chars().map(|c| (c as u32).to_string()).collect();
                format!("EV paste {}", cps.join(" "))
            }
            Fev::Copy => "EV copy".into(),
            Fev::Cut => "EV cut".into(),
            Fev::Resize { w, h } => format!("EV resize {w:.1} {h:.1}"),
            Fev::Zoom { f } => format!("EV zoom {f:.3}"),
        }
    }

    fn parse(line: &str) -> Option<Fev> {
        let t = line.strip_prefix("EV ")?;
        let mut p = t.split_whitespace();
        match p.next()? {
            "move" => Some(Fev::Move {
                x: p.next()?.parse().ok()?,
                y: p.next()?.parse().ok()?,
            }),
            "down" => Some(Fev::Down {
                x: p.next()?.parse().ok()?,
                y: p.next()?.parse().ok()?,
                btn: Btn::from_idx(p.next()?.parse().ok()?),
                mods: p.next()?.parse().ok()?,
            }),
            "up" => Some(Fev::Up {
                x: p.next()?.parse().ok()?,
                y: p.next()?.parse().ok()?,
                btn: Btn::from_idx(p.next()?.parse().ok()?),
                mods: p.next()?.parse().ok()?,
            }),
            "wheel" => Some(Fev::Wheel {
                x: p.next()?.parse().ok()?,
                y: p.next()?.parse().ok()?,
                dx: p.next()?.parse().ok()?,
                dy: p.next()?.parse().ok()?,
                unit: p.next()?.parse().ok()?,
            }),
            "key" => {
                let name = p.next()?;
                let pressed: u8 = p.next()?.parse().ok()?;
                let mods: u8 = p.next()?.parse().ok()?;
                // All generator-emitted key names come from KEY_TABLE, so the
                // static entry can be reused (no leaking on replay).
                let key: &'static str = KEY_TABLE
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(n, _)| *n)
                    .unwrap_or("Space");
                Some(Fev::Key {
                    key,
                    pressed: pressed != 0,
                    mods,
                })
            }
            "text" => {
                let cp: u32 = p.next()?.parse().ok()?;
                Some(Fev::Text(char::from_u32(cp).unwrap_or('?')))
            }
            "paste" => {
                let s: String = p
                    .map(|c| c.parse::<u32>().ok().and_then(char::from_u32).unwrap_or('?'))
                    .collect();
                Some(Fev::Paste(s))
            }
            "copy" => Some(Fev::Copy),
            "cut" => Some(Fev::Cut),
            "resize" => Some(Fev::Resize {
                w: p.next()?.parse().ok()?,
                h: p.next()?.parse().ok()?,
            }),
            "zoom" => Some(Fev::Zoom {
                f: p.next()?.parse().ok()?,
            }),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Event generator: weighted toward real usage
// ---------------------------------------------------------------------------

/// Key names the generator can press. Letters/digits carry their char for
/// optional paired Text events.
const KEY_TABLE: &[(&str, Option<char>)] = &[
    ("a", Some('a')), ("b", Some('b')), ("c", Some('c')), ("d", Some('d')),
    ("e", Some('e')), ("f", Some('f')), ("g", Some('g')), ("h", Some('h')),
    ("i", Some('i')), ("j", Some('j')), ("k", Some('k')), ("l", Some('l')),
    ("m", Some('m')), ("n", Some('n')), ("o", Some('o')), ("p", Some('p')),
    ("q", Some('q')), ("r", Some('r')), ("s", Some('s')), ("t", Some('t')),
    ("u", Some('u')), ("v", Some('v')), ("w", Some('w')), ("x", Some('x')),
    ("y", Some('y')), ("z", Some('z')),
    ("0", Some('0')), ("1", Some('1')), ("2", Some('2')), ("3", Some('3')),
    ("4", Some('4')), ("5", Some('5')), ("6", Some('6')), ("7", Some('7')),
    ("8", Some('8')), ("9", Some('9')),
    ("Enter", None), ("Escape", None), ("Tab", None), ("Backspace", None),
    ("Delete", None), ("Space", Some(' ')),
    ("ArrowUp", None), ("ArrowDown", None), ("ArrowLeft", None), ("ArrowRight", None),
    ("Home", None), ("End", None), ("PageUp", None), ("PageDown", None),
    ("Backtick", Some('`')),
    ("F1", None), ("F2", None), ("F3", None), ("F4", None), ("F5", None),
    ("F6", None), ("F7", None), ("F8", None), ("F9", None), ("F10", None),
    ("F11", None), ("F12", None),
];

const TEXT_POOL: &[char] = &[
    'a', 'e', 'i', 'o', 'n', 'r', 't', 'l', 's', ' ', '0', '1', '2', '_', '-', '.',
    '/', '(', ')', '=', '+', 'A', 'Z', '9', 'x', ';', ':', '\'',
    // A few non-ASCII codepoints to exercise font fallback paths.
    '\u{e9}', '\u{4e2d}', '\u{3c0}', '\u{221e}',
];

struct Gen {
    rng: Rng,
    w: f32,
    h: f32,
    px: f32,
    py: f32,
}

impl Gen {
    fn new(seed: u64, w: f32, h: f32) -> Self {
        Self {
            rng: Rng::new(seed),
            w,
            h,
            px: w / 2.0,
            py: h / 2.0,
        }
    }

    /// Position biased toward real chrome: menu bar, activity rail, sidebar,
    /// bottom panel, central canvas; sometimes uniform; sometimes an edge.
    fn pos(&mut self) -> (f32, f32) {
        let r = self.rng.below_u64(100);
        let (x, y) = if r < 8 {
            // Menu bar strip.
            (self.rng.range_f(4.0, self.w - 4.0), self.rng.range_f(4.0, 26.0))
        } else if r < 16 {
            // Activity rail.
            (self.rng.range_f(4.0, 84.0), self.rng.range_f(30.0, self.h - 30.0))
        } else if r < 26 {
            // Sidebar column.
            (self.rng.range_f(92.0, 300.0), self.rng.range_f(30.0, self.h - 190.0))
        } else if r < 36 {
            // Bottom panel.
            (
                self.rng.range_f(92.0, self.w - 4.0),
                self.rng.range_f((self.h - 200.0).max(40.0), self.h - 8.0),
            )
        } else if r < 62 {
            // Central canvas.
            (
                self.rng.range_f(310.0, (self.w - 8.0).max(320.0)),
                self.rng.range_f(34.0, (self.h - 210.0).max(60.0)),
            )
        } else if r < 66 {
            // Edges and corners (resize handles live here).
            let ex = *self.rng.pick(&[2.0, self.w - 2.0, self.w / 2.0]);
            let ey = *self.rng.pick(&[2.0, self.h - 2.0, self.h / 2.0]);
            (ex, ey)
        } else {
            // Uniform.
            (
                self.rng.range_f(0.0, self.w),
                self.rng.range_f(0.0, self.h),
            )
        };
        self.px = x;
        self.py = y;
        (x, y)
    }

    fn mods(&mut self) -> u8 {
        let r = self.rng.below_u64(100);
        if r < 70 {
            0
        } else if r < 80 {
            1 // shift
        } else if r < 88 {
            2 | 8 // ctrl + command (Linux: both reach shortcuts)
        } else if r < 93 {
            8 // command only
        } else if r < 97 {
            4 // alt
        } else {
            1 | 2 // shift+ctrl
        }
    }

    fn btn(&mut self) -> Btn {
        match self.rng.below_u64(100) {
            0..=84 => Btn::Primary,
            85..=94 => Btn::Secondary,
            _ => Btn::Middle,
        }
    }

    fn click_at(&mut self, x: f32, y: f32, out: &mut Vec<Fev>) {
        let b = self.btn();
        let m = self.mods();
        out.push(Fev::Move { x, y });
        out.push(Fev::Down { x, y, btn: b, mods: m });
        if self.rng.chance(14) {
            // Double-click: second press/release quickly.
            out.push(Fev::Up { x, y, btn: b, mods: m });
            out.push(Fev::Down { x, y, btn: b, mods: m });
        }
        out.push(Fev::Up { x, y, btn: b, mods: m });
    }

    fn key_cluster(&mut self, out: &mut Vec<Fev>) {
        let n = 1 + self.rng.below(3);
        for _ in 0..n {
            let (name, ch) = *self.rng.pick(KEY_TABLE);
            // Shortcut-relevant keys get command-modifier more often so
            // handle_shortcuts paths (Cmd+O/P/K/J/1/2/3/Enter/S/`) are hit.
            let shortcut_key = matches!(name, "o" | "p" | "k" | "s" | "j" | "z" | "Enter" | "1" | "2" | "3" | "Backtick");
            let mods = if shortcut_key && self.rng.chance(45) {
                8
            } else {
                self.mods()
            };
            out.push(Fev::Key { key: name, pressed: true, mods });
            // Real integrations also send Text for printable keys.
            if let Some(c) = ch {
                if mods & (2 | 8) == 0 && self.rng.chance(70) {
                    let c = if mods & 1 != 0 { c.to_ascii_uppercase() } else { c };
                    out.push(Fev::Text(c));
                }
            }
            if self.rng.chance(85) {
                out.push(Fev::Key { key: name, pressed: false, mods });
            }
        }
    }

    /// Generate one frame's worth of fuzz events.
    fn frame_events(&mut self) -> Vec<Fev> {
        let mut out = Vec::new();
        let r = self.rng.below_u64(100);
        if r < 32 {
            let (x, y) = self.pos();
            self.click_at(x, y, &mut out);
            // Sometimes keep clicking around the same spot (menu drill-down).
            if self.rng.chance(25) {
                let (x2, y2) = (x + self.rng.range_f(-40.0, 40.0), y + self.rng.range_f(-10.0, 60.0));
                let x2 = x2.clamp(0.0, self.w);
                let y2 = y2.clamp(0.0, self.h);
                self.click_at(x2, y2, &mut out);
            }
        } else if r < 42 {
            // Drag.
            let (x0, y0) = self.pos();
            let (x1, y1) = self.pos();
            let b = self.btn();
            let m = self.mods();
            out.push(Fev::Move { x: x0, y: y0 });
            out.push(Fev::Down { x: x0, y: y0, btn: b, mods: m });
            let steps = 2 + self.rng.below(5);
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                out.push(Fev::Move {
                    x: x0 + (x1 - x0) * t,
                    y: y0 + (y1 - y0) * t,
                });
            }
            out.push(Fev::Up { x: x1, y: y1, btn: b, mods: m });
        } else if r < 56 {
            // Scroll (both directions; shift-scroll sometimes for horizontal).
            let (x, y) = self.pos();
            let nw = 1 + self.rng.below(3);
            for _ in 0..nw {
                let dir = if self.rng.chance(50) { 1.0 } else { -1.0 };
                let horiz = self.rng.chance(20);
                let mag = self.rng.range_f(20.0, 240.0) * dir;
                let (dx, dy) = if horiz { (mag, 0.0) } else { (0.0, mag) };
                let unit = self.rng.below(3) as u8;
                out.push(Fev::Wheel { x, y, dx, dy, unit });
            }
        } else if r < 70 {
            self.key_cluster(&mut out);
        } else if r < 80 {
            let n = 1 + self.rng.below(3);
            for _ in 0..n {
                let (x, y) = self.pos();
                out.push(Fev::Move { x, y });
            }
        } else if r < 86 {
            let n = 1 + self.rng.below(8);
            for _ in 0..n {
                out.push(Fev::Text(*self.rng.pick(TEXT_POOL)));
            }
        } else if r < 90 {
            match self.rng.below(3) {
                0 => out.push(Fev::Copy),
                1 => out.push(Fev::Cut),
                _ => {
                    let n = 1 + self.rng.below(12);
                    let s: String = (0..n).map(|_| *self.rng.pick(TEXT_POOL)).collect();
                    out.push(Fev::Paste(s));
                }
            }
        } else if r < 93 {
            // Resize: mostly sane, sometimes extreme.
            let (w, h) = if self.rng.chance(80) {
                (self.rng.range_f(720.0, 1920.0), self.rng.range_f(400.0, 1080.0))
            } else {
                (
                    *self.rng.pick(&[320.0, 500.0, 2560.0, 3840.0]),
                    *self.rng.pick(&[200.0, 300.0, 1440.0, 2160.0]),
                )
            };
            self.w = w;
            self.h = h;
            out.push(Fev::Resize { w, h });
        } else if r < 95 {
            out.push(Fev::Zoom { f: self.rng.range_f(0.5, 2.0) });
        } else {
            // Click with the pointer already somewhere (uses last pos).
            let (x, y) = (self.px, self.py);
            self.click_at(x, y, &mut out);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Frame driver: the fuzzer calls HelionIde::update_inner directly - the exact
// same frame path as `eframe::App::update`, with no duplication.
// ---------------------------------------------------------------------------

/// Run one headless frame through the real update path, catching panics.
/// Returns egui's FullOutput, or the panic payload.
fn pump_frame(
    ctx: &egui::Context,
    app: &mut HelionIde,
    raw: egui::RawInput,
) -> Result<egui::FullOutput, Box<dyn std::any::Any + Send>> {
    IN_FRAME.set(true);
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| ctx.run(raw, |c| app.update_inner(c))));
    IN_FRAME.set(false);
    res
}

// ---------------------------------------------------------------------------
// Invariant snapshots
// ---------------------------------------------------------------------------

/// Values that selection-only UI events must never change. Compared before
/// and after every frame; a change with no job/clock/constraint activity is
/// logged as an anomaly (this is the "mouse on timing view mutates timing
/// values" bug class).
#[derive(Clone, Debug, PartialEq)]
struct TimingSnap {
    wns_ps: i64,
    tns_ps: i64,
    endpoints: usize,
    clocks: usize,
    r2r_ps: i64,
    iob_ps: i64,
    setup_ps: i64,
    hold_ps: i64,
    hold_slack_ps: i64,
    route_ps: i64,
    clk_net_ps: i64,
    clock_period_ps: u64,
    steps: [StepState; 5],
}

#[derive(Clone, Debug)]
struct ModelSnap {
    timing: Option<TimingSnap>,
    device_zoom: f32,
    sidebar_width: f32,
    console_height: f32,
    selected_timing_path: Option<usize>,
    timing_paths_len: usize,
    busy: bool,
    job_active: bool,
    selected_breakpoint: Option<usize>,
    breakpoints_len: usize,
    /// Console/trace growth hints at synchronous Tcl `exec` (which can
    /// legitimately recompute timing with no job in flight).
    console_len: usize,
    input_history_len: usize,
    /// Design-run status changes (a run finishing/activating can also swap
    /// timing in synchronously via `sync_from_session`).
    runs_sig: Vec<(String, String)>,
}

fn snapshot(app: &HelionIde) -> ModelSnap {
    let timing = app.model.timing.as_ref().map(|t| TimingSnap {
        wns_ps: t.wns_ps,
        tns_ps: t.tns_ps,
        endpoints: t.endpoints,
        clocks: t.clocks.len(),
        r2r_ps: t.r2r_ps,
        iob_ps: t.iob_ps,
        setup_ps: t.setup_ps,
        hold_ps: t.hold_ps,
        hold_slack_ps: t.hold_slack_ps,
        route_ps: t.route_ps,
        clk_net_ps: t.clk_net_ps,
        clock_period_ps: app.model.clock_period_ps,
        steps: app.model.steps,
    });
    ModelSnap {
        timing,
        device_zoom: app.model.device_zoom,
        sidebar_width: app.sidebar_width,
        console_height: app.console_height,
        selected_timing_path: app.model.selected_timing_path,
        timing_paths_len: app.model.timing_paths.len(),
        busy: app.busy,
        job_active: app.job.is_some(),
        selected_breakpoint: app.model.selected_breakpoint,
        breakpoints_len: app.model.breakpoints.len(),
        console_len: app.model.console.len(),
        input_history_len: app.model.input_history.len(),
        runs_sig: app
            .model
            .runs
            .iter()
            .map(|r| (r.name.clone(), r.status.clone()))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Logging, panic hook, config
// ---------------------------------------------------------------------------

static LOG_FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
static CRASH_DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

thread_local! {
    /// Set while a fuzz frame runs on this thread: the panic hook skips
    /// logging because the frame handler logs with frame context.
    static IN_FRAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Backtrace captured by the panic hook at the panic site. The hook runs
    /// on the panicking thread before unwinding, so this is the true panic
    /// stack; capturing in the frame handler instead would only see the
    /// handler's own stack and every crash would share one signature.
    static PANIC_BT: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

fn take_panic_bt() -> Option<String> {
    PANIC_BT.with(|c| c.borrow_mut().take())
}

fn log_line(s: &str) {
    if let Some(f) = LOG_FILE.get() {
        if let Ok(mut f) = f.lock() {
            let _ = writeln!(f, "{s}");
        }
    }
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);
        // Capture at the panic site, on the panicking thread, before
        // unwinding: this is the true stack. The frame handler consumes it
        // via `take_panic_bt` for crash signatures.
        let bt = format!("{:?}", std::backtrace::Backtrace::force_capture());
        PANIC_BT.with(|c| *c.borrow_mut() = Some(bt.clone()));
        if IN_FRAME.get() {
            return;
        }
        let payload = info
            .payload()
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| info.payload().downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "<non-string panic payload>".into());
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".into());
        log_line(&format!(
            "BG-PANIC thread={:?} at {loc}: {payload}\nbacktrace:\n{bt}",
            std::thread::current().name()
        ));
    }));
}

#[derive(Clone)]
struct Config {
    seeds: Vec<u64>,
    frames: usize,
    deadline_secs: u64,
    logdir: std::path::PathBuf,
    replay: Option<std::path::PathBuf>,
    design: Option<std::path::PathBuf>,
    warmup: bool,
    quiet: bool,
}

fn env_u64(name: &str, def: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(def)
}
fn env_usize(name: &str, def: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(def)
}
fn env_bool(name: &str, def: bool) -> bool {
    match std::env::var(name).ok().as_deref() {
        Some("1") | Some("true") | Some("yes") => true,
        Some("0") | Some("false") | Some("no") => false,
        _ => def,
    }
}

fn parse_seeds(extra: &str, base: u64) -> Vec<u64> {
    let mut out = vec![base];
    for part in extra.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<u64>(), b.trim().parse::<u64>()) {
                for s in a..=b {
                    out.push(s);
                }
                continue;
            }
        }
        if let Ok(s) = part.parse::<u64>() {
            out.push(s);
        }
    }
    out
}

impl Config {
    fn from_env() -> Self {
        let seed = env_u64("HELION_FUZZ_SEED", 0x9E3779B97F4A7C15);
        let seeds = parse_seeds(&std::env::var("HELION_FUZZ_SEEDS").unwrap_or_default(), seed);
        let logdir = std::env::var("HELION_FUZZ_LOGDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join("fuzz_runs"));
        let design = match std::env::var("HELION_FUZZ_DESIGN").ok().as_deref() {
            None => resolve_design("examples/blinky.sv"),
            Some("") => None,
            Some(p) => resolve_design(p),
        };
        Self {
            seeds,
            frames: env_usize("HELION_FUZZ_FRAMES", 20000),
            deadline_secs: env_u64("HELION_FUZZ_DEADLINE_SECS", 0),
            logdir,
            replay: std::env::var("HELION_FUZZ_REPLAY").ok().map(std::path::PathBuf::from),
            design,
            warmup: env_bool("HELION_FUZZ_WARMUP", true),
            quiet: env_bool("HELION_FUZZ_QUIET", false),
        }
    }
}

/// Scrub env knobs that would make runs nondeterministic or pop UI.
fn sanitize_env() {
    // SAFETY: called once at harness startup, before any engine/job thread
    // exists; no other thread can observe the environment concurrently.
    unsafe {
        for k in [
            "HELION_OPEN",
            "HELION_FLOW",
            "HELION_SHOTDIR",
            "HELION_SHOT",
            "HELION_SHOT_QUIT",
            "HELION_AUTO_CHECKPOINT",
            "HELION_WORKSPACE",
            "HELION_DEBUG_OVERLAY",
            "HELION_UI_LOG",
            "HELION_SIDEBAR_WIDTH",
            "HELION_CONSOLE_HEIGHT",
        ] {
            std::env::remove_var(k);
        }
        // rfd's xdg-portal backend would otherwise try D-Bus autolaunch; point it
        // at a dead socket so file dialogs fail fast and headless instead of
        // hanging the fuzz loop.
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-helion-fuzz-bus");
    }
}

fn say(cfg: &Config, s: &str) {
    if !cfg.quiet {
        println!("{s}");
    }
}

// ---------------------------------------------------------------------------
// App setup + warmup
// ---------------------------------------------------------------------------

fn setup_app() -> (egui::Context, HelionIde) {
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());
    theme::install_fonts(&ctx);
    ctx.style_mut(|s| {
        s.interaction.resize_grab_radius_side = chrome::SPLITTER_GRAB_PX;
        s.interaction.resize_grab_radius_corner = chrome::SPLITTER_GRAB_PX + 4.0;
    });
    (ctx, HelionIde::new())
}

fn raw_input(w: f32, h: f32, t: f64, events: Vec<egui::Event>) -> egui::RawInput {
    // NOTE: egui 0.32 RawInput does not implement Default; list every field.
    // The `viewports` map must contain the active viewport or `Context::run`
    // panics with "Failed to find current viewport".
    let mut viewports = egui::ViewportIdMap::default();
    viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo::default());
    egui::RawInput {
        viewport_id: egui::ViewportId::ROOT,
        viewports,
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(w, h),
        )),
        max_texture_side: None,
        time: Some(t),
        predicted_dt: 1.0 / 60.0,
        modifiers: egui::Modifiers::default(),
        events,
        hovered_files: Vec::new(),
        dropped_files: Vec::new(),
        focused: true,
        system_theme: None,
    }
}

/// Open the warmup design and run Implement so the timing/device views hold
/// real data for the fuzzer to poke at. Engine work happens on the job thread;
/// frames keep pumping (empty input) until the job merges or times out.
/// Returns the warmup panic payload, if the warmup itself panicked.
fn warmup(
    ctx: &egui::Context,
    app: &mut HelionIde,
    cfg: &Config,
    design: Option<&std::path::Path>,
) -> Option<String> {
    if let Some(design) = design {
        if design.is_file() {
            app.open_path(design);
            say(cfg, &format!("fuzz: warmup opened {}", design.display()));
        } else {
            say(cfg, &format!("fuzz: warmup design missing: {}", design.display()));
        }
    }
    // Let first-paint init settle.
    for i in 0..5 {
        let raw = raw_input(1440.0, 900.0, i as f64 / 60.0, vec![]);
        let _ = pump_frame(ctx, app, raw);
    }
    if !cfg.warmup {
        return None;
    }
    app.submit_job(surface::JobKind::Implement);
    let t0 = Instant::now();
    let mut t = 5.0 / 60.0;
    let mut frames = 0u32;
    let mut warmup_panic: Option<String> = None;
    while app.job.is_some() {
        t += 1.0 / 60.0;
        let raw = raw_input(1440.0, 900.0, t, vec![]);
        if let Err(p) = pump_frame(ctx, app, raw) {
            warmup_panic = Some(panic_payload_str(&p));
            break;
        }
        frames += 1;
        if t0.elapsed().as_secs() > 300 || frames > 30_000 {
            log_line(&format!("WARMUP-TIMEOUT frames={frames}"));
            say(cfg, "fuzz: warmup Implement timed out; continuing anyway");
            break;
        }
    }
    match warmup_panic {
        Some(p) => {
            log_line(&format!("WARMUP-PANIC: {p}"));
            say(cfg, &format!("fuzz: warmup panicked: {p}"));
            Some(p)
        }
        None => {
            say(
                cfg,
                &format!(
                    "fuzz: warmup Implement done in {:.1}s ({frames} pump frames)",
                    t0.elapsed().as_secs_f32()
                ),
            );
            None
        }
    }
}

fn panic_payload_str(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "<non-string panic payload>".into())
}

/// Short signature for crash dedup: payload + top app frames of the backtrace.
/// Uses only stable APIs: `Backtrace::frames()` is still unstable on this
/// toolchain, so symbol names are scraped from the stable `Debug` format
/// (`Backtrace [{ fn: "a::b" }, ...]`). The backtrace comes from
/// `take_panic_bt` (captured by the hook at the panic site); without it the
/// handler would only see its own stack.
fn panic_sig(p: &Box<dyn std::any::Any + Send>, bt_dbg: &str) -> String {
    let msg = panic_payload_str(p);
    let mut frames: Vec<String> = Vec::new();
    let mut rest = bt_dbg;
    while let Some(i) = rest.find("fn: \"") {
        rest = &rest[i + 5..];
        let Some(end) = rest.find('"') else { break };
        let name = &rest[..end];
        rest = &rest[end..];
        if name.contains("fuzz::")
            || name.contains("core::panicking")
            || name.contains("std::panicking")
            || name.contains("__rust_begin_short_backtrace")
            || name.contains("lang_start")
        {
            continue;
        }
        frames.push(name.to_string());
        if frames.len() >= 6 {
            break;
        }
    }
    format!("{msg}\n{}", frames.join("\n"))
}

/// Triaged panics the fuzzer pushes past instead of aborting on, so a seed
/// can reach deeper code after hitting a known shallow bug. Matched on
/// (payload substring, backtrace-frame substring) so value variations (e.g.
/// different window heights in a clamp message) and different UI paths to
/// the same assertion all count as known. Keep in sync with the regression
/// tests in `crash_regression` below.
fn is_known_panic(payload: &str, bt_dbg: &str) -> bool {
    const KNOWN: &[(&str, &str)] = &[
        // chrome::device_band_share clamps (64.0, after) with after < 64 on
        // very short windows.
        ("min > max, or either was NaN", "device_band_share"),
        // open_more_workspace debug assertion: apply_more(Device) sets
        // workspace but leaves canvas stale.
        ("updated workspace but painted", "open_more_workspace"),
    ];
    KNOWN
        .iter()
        .any(|(p, f)| payload.contains(p) && bt_dbg.contains(f))
}

fn sighash(sig: &str) -> String {
    // FNV-1a 64, hex, 12 chars.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in sig.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:012x}")
}

// ---------------------------------------------------------------------------
// Invariant checking
// ---------------------------------------------------------------------------

struct Checker {
    busy_stuck: u32,
    dlg: HashMap<&'static str, (u32, u32)>, // name -> (opens, closes)
    prev_dlg: HashMap<&'static str, bool>,
}

impl Checker {
    fn new() -> Self {
        Self {
            busy_stuck: 0,
            dlg: HashMap::new(),
            prev_dlg: HashMap::new(),
        }
    }

    fn dialogs(app: &HelionIde) -> [(&'static str, bool); 5] {
        [
            ("palette", app.palette.open),
            ("cheatsheet", app.show_cheatsheet),
            ("examples", app.show_examples),
            ("create_project", app.show_create_project),
            ("tcl", app.show_tcl),
        ]
    }

    fn note_dialogs(&mut self, app: &HelionIde) {
        for (name, open) in Self::dialogs(app) {
            let was = self.prev_dlg.get(name).copied().unwrap_or(false);
            if open && !was {
                self.dlg.entry(name).or_insert((0, 0)).0 += 1;
            } else if !open && was {
                self.dlg.entry(name).or_insert((0, 0)).1 += 1;
            }
            self.prev_dlg.insert(name, open);
        }
    }

    fn check(
        &mut self,
        cfg: &Config,
        frame: usize,
        win_w: f32,
        win_h: f32,
        before: &ModelSnap,
        app: &HelionIde,
        fevs: &[Fev],
        anomalies: &mut HashMap<&'static str, usize>,
    ) {
        let mut hit = |kind: &'static str, detail: String| {
            *anomalies.entry(kind).or_insert(0) += 1;
            let ev: Vec<String> = fevs.iter().map(|e| e.serialize()).collect();
            log_line(&format!("ANOMALY {kind} frame={frame} {detail} events=[{}]", ev.join(" | ")));
            if !cfg.quiet {
                println!("fuzz anomaly {kind} @frame {frame}: {detail}");
            }
        };
        let after = snapshot(app);

        // 1. Timing values mutated with no job/clock activity (the reported
        // bug class: "mouse on timing view mutates timing values").
        match (&before.timing, &after.timing) {
            (Some(b), Some(a)) if b != a => {
                let job_activity = before.job_active || after.job_active || before.busy != after.busy;
                // Synchronous Tcl exec (typed into the console) can also
                // recompute timing with no job in flight; note it for triage.
                let tcl_activity = after.console_len != before.console_len
                    || after.input_history_len != before.input_history_len;
                let runs_activity = after.runs_sig != before.runs_sig;
                if !job_activity && !tcl_activity && !runs_activity {
                    hit("TIMING-MUTATED", format!("{b:?} -> {a:?}"));
                } else if !job_activity {
                    hit(
                        "TIMING-MUTATED-EXPLAINABLE",
                        format!("{b:?} -> {a:?} (tcl={tcl_activity} runs={runs_activity})"),
                    );
                }
            }
            _ => {}
        }

        // 2. Non-finite / out-of-range geometry state.
        for (name, v) in [
            ("device_zoom", after.device_zoom),
            ("sidebar_width", after.sidebar_width),
            ("console_height", after.console_height),
        ] {
            if !v.is_finite() {
                hit("NONFINITE", format!("{name}={v}"));
            }
        }
        if after.device_zoom <= 0.0 {
            hit("BOUNDS", format!("device_zoom={}", after.device_zoom));
        }
        // The app clamps both to share_available() at the top of every frame
        // (apply_shared_chrome), but a drag on the resize handle can push the
        // value past the cap for the rest of THAT frame - the panel itself has
        // no max. So the tight [floor, cap] mirror is noise; the real sanity
        // contract is: finite, never below the floor, never beyond the window.
        let share = chrome::share_available(win_w, win_h);
        if !(share.sidebar_floor - 1.0..=win_w + 1.0).contains(&after.sidebar_width) {
            hit(
                "BOUNDS",
                format!(
                    "sidebar_width={} outside sane [{}, {}] (win_w={win_w})",
                    after.sidebar_width,
                    share.sidebar_floor - 1.0,
                    win_w + 1.0
                ),
            );
        }
        if !(share.console_floor - 1.0..=win_h + 1.0).contains(&after.console_height) {
            hit(
                "BOUNDS",
                format!(
                    "console_height={} outside sane [{}, {}] (win_h={win_h})",
                    after.console_height,
                    share.console_floor - 1.0,
                    win_h + 1.0
                ),
            );
        }

        // 3. Selected indices out of range.
        if let Some(i) = after.selected_timing_path {
            if i >= after.timing_paths_len {
                hit("SELECT-OOR", format!("selected_timing_path={i} len={}", after.timing_paths_len));
            }
        }
        if let Some(i) = after.selected_breakpoint {
            if i >= after.breakpoints_len {
                hit("SELECT-OOR", format!("selected_breakpoint={i} len={}", after.breakpoints_len));
            }
        }

        // 4. Busy spinner stuck with no job behind it.
        if after.busy && !after.job_active {
            self.busy_stuck += 1;
            if self.busy_stuck == 600 {
                hit("BUSY-STUCK", "busy=true with no job for 600 frames".into());
            }
        } else {
            self.busy_stuck = 0;
        }

        self.note_dialogs(app);
    }
}

// ---------------------------------------------------------------------------
// One fuzz seed
// ---------------------------------------------------------------------------

struct SeedStats {
    frames: usize,
    events: usize,
    panics: usize,
    aborted: bool,
    anomalies: HashMap<&'static str, usize>,
    /// Crash replay files written by this seed (for post-run minimization).
    crashes: Vec<std::path::PathBuf>,
}

fn flush_seed_log(cfg: &Config, seed: u64, header: &[String], frames_log: &[String]) {
    let path = cfg.logdir.join(format!("fuzz-s{seed}.log"));
    let _ = std::fs::create_dir_all(&cfg.logdir);
    if let Ok(mut f) = std::fs::File::create(&path) {
        for l in header.iter().chain(frames_log.iter()) {
            let _ = writeln!(f, "{l}");
        }
    }
    say(cfg, &format!("fuzz: seed {seed} event log -> {}", path.display()));
}

fn crash_dir() -> std::path::PathBuf {
    CRASH_DIR
        .get_or_init(|| {
            let d = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz_crashes");
            let _ = std::fs::create_dir_all(&d);
            d
        })
        .clone()
}

/// Write the crashing prefix (frames 0..=crash_frame) as a replay log.
/// Returns the path written.
fn write_crash_log(sigh: &str, header: &[String], frames_log: &[String], crash_frame: usize) -> std::path::PathBuf {
    // frames_log holds per-frame blocks; find the block boundaries.
    let mut out: Vec<String> = header.to_vec();
    let mut cur = 0usize;
    for line in frames_log {
        if line.starts_with("FRAME ") {
            if cur > crash_frame {
                break;
            }
            cur += 1;
        }
        out.push(line.clone());
    }
    let path = crash_dir().join(format!("crash-{sigh}.log"));
    let _ = std::fs::write(&path, out.join("\n"));
    path
}

fn run_seed(cfg: &Config, seed: u64) -> SeedStats {
    let (ctx, mut app) = setup_app();
    let design = cfg.design.clone();

    let mut stats = SeedStats {
        frames: 0,
        events: 0,
        panics: 0,
        aborted: false,
        anomalies: HashMap::new(),
        crashes: Vec::new(),
    };
    let mut seen: HashSet<String> = HashSet::new();

    // Buffered per-frame log lines; flushed to the run log at seed end (and
    // used for crash-prefix extraction).
    let mut frames_log: Vec<String> = Vec::with_capacity(cfg.frames * 2);
    let header = vec![
        "FUZZLOG v1".to_string(),
        format!("seed={seed}"),
        format!(
            "design={}",
            design.as_deref().map(|d| d.display().to_string()).unwrap_or_default()
        ),
        format!("warmup={}", cfg.warmup as u8),
    ];

    if let Some(p) = warmup(&ctx, &mut app, cfg, design.as_deref()) {
        stats.panics += 1;
        let sig = format!("WARMUP-PANIC\n{p}");
        if seen.insert(sig.clone()) {
            let path = write_crash_log(&sighash(&sig), &header, &frames_log, 0);
            say(cfg, &format!("fuzz: warmup crash replay written to {}", path.display()));
            stats.crashes.push(path);
        }
        flush_seed_log(cfg, seed, &header, &frames_log);
        stats.aborted = true;
        return stats;
    }

    let mut evgen = Gen::new(seed, 1440.0, 900.0);
    let mut checker = Checker::new();

    let run_t0 = Instant::now();
    for frame in 0..cfg.frames {
        let fevs = evgen.frame_events();
        let (w, h) = (evgen.w, evgen.h);
        let t = frame as f64 / 60.0;
        let before = snapshot(&app);

        let mut egui_events = Vec::new();
        for fev in &fevs {
            egui_events.extend(fev.to_egui());
        }
        let raw = raw_input(w, h, t, egui_events);

        frames_log.push(format!("FRAME {frame} rect={w:.1}x{h:.1} t={t:.6}"));
        for fev in &fevs {
            frames_log.push(fev.serialize());
        }
        stats.events += fevs.len();

        let ft0 = Instant::now();
        let res = pump_frame(&ctx, &mut app, raw);
        let fdt = ft0.elapsed();

        match res {
            Ok(output) => {
                stats.frames += 1;
                checker.check(cfg, frame, w, h, &before, &app, &fevs, &mut stats.anomalies);
                if fdt.as_secs_f32() > 10.0 {
                    *stats.anomalies.entry("FREEZE").or_insert(0) += 1;
                    let ev: Vec<String> = fevs.iter().map(|e| e.serialize()).collect();
                    log_line(&format!(
                        "ANOMALY FREEZE frame={frame} dt={:.1}s events=[{}]",
                        fdt.as_secs_f32(),
                        ev.join(" | ")
                    ));
                }
                if output.shapes.len() > 1_000_000 {
                    *stats.anomalies.entry("SHAPES").or_insert(0) += 1;
                    log_line(&format!("ANOMALY SHAPES frame={frame} n={}", output.shapes.len()));
                }
            }
            Err(p) => {
                stats.panics += 1;
                let payload = panic_payload_str(&p);
                // True panic-site backtrace, stashed by the hook (see
                // take_panic_bt); capturing here would only see this handler.
                let bt_dbg = take_panic_bt().unwrap_or_default();
                let sig = panic_sig(&p, &bt_dbg);
                let sigh = sighash(&sig);
                let ev: Vec<String> = fevs.iter().map(|e| e.serialize()).collect();
                log_line(&format!(
                    "PANIC frame={frame} sigh={sigh} payload={payload}\nevents=[{}]\nbacktrace:\n{bt_dbg}",
                    ev.join(" | ")
                ));
                say(cfg, &format!("fuzz PANIC @frame {frame} sigh={sigh}: {payload}"));
                if is_known_panic(&payload, &bt_dbg) {
                    // Triaged already: count it and keep fuzzing past it so the
                    // seed can reach deeper code. The replay path skips known
                    // panics the same way, so a later NEW crash still gets a
                    // clean replay prefix.
                    *stats.anomalies.entry("KNOWN-PANIC").or_insert(0) += 1;
                    log_line(&format!("KNOWN-PANIC frame={frame} sigh={sigh}"));
                } else {
                    if seen.insert(sig) {
                        let path = write_crash_log(&sigh, &header, &frames_log, frame);
                        say(cfg, &format!("fuzz: crash replay written to {}", path.display()));
                        stats.crashes.push(path);
                    }
                    // Stop this seed after a NEW panic: continuing on a
                    // half-unwound app/ctx would pollute the replay prefix (a
                    // later crash's log would contain this earlier panicking
                    // frame). The outer loop moves on to the next seed.
                    flush_seed_log(cfg, seed, &header, &frames_log);
                    stats.aborted = true;
                    break;
                }
            }
        }

        if frame % 2000 == 0 && frame > 0 && !cfg.quiet {
            println!(
                "fuzz seed={seed} frame={frame}/{n} events={} panics={} elapsed={:.0}s",
                stats.events,
                stats.panics,
                run_t0.elapsed().as_secs_f32(),
                n = cfg.frames,
            );
        }
        // Flush the event log periodically so a hang or kill never loses the
        // prefix needed to reproduce it.
        if frame % 500 == 499 {
            flush_seed_log(cfg, seed, &header, &frames_log);
        }
    }

    // Flush this seed's full event log.
    flush_seed_log(cfg, seed, &header, &frames_log);

    // Dialog open/close balance for stuck-state triage.
    for (name, (opens, closes)) in &checker.dlg {
        log_line(&format!("DIALOG seed={seed} {name}: opens={opens} closes={closes}"));
    }

    stats
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

struct ReplayFrame {
    w: f32,
    h: f32,
    t: f64,
    events: Vec<Fev>,
}

struct ReplayLog {
    design: Option<std::path::PathBuf>,
    warmup: bool,
    frames: Vec<ReplayFrame>,
}

fn resolve_design(given: &str) -> Option<std::path::PathBuf> {
    if given.is_empty() {
        return None;
    }
    let p = std::path::PathBuf::from(given);
    if p.is_file() {
        return Some(p);
    }
    // Workspace root relative to this crate's manifest dir.
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(given);
    if ws.is_file() {
        return Some(ws);
    }
    None
}

fn parse_replay(text: &str) -> Option<ReplayLog> {
    let mut design: Option<String> = None;
    let mut warmup = true;
    let mut frames: Vec<ReplayFrame> = Vec::new();
    for line in text.lines() {
        if let Some(d) = line.strip_prefix("design=") {
            design = Some(d.to_string());
            continue;
        }
        if let Some(w) = line.strip_prefix("warmup=") {
            warmup = w.trim() == "1";
            continue;
        }
        if let Some(rest) = line.strip_prefix("FRAME ") {
            // FRAME <n> rect=<w>x<h> t=<t>
            let mut w = 1440.0f32;
            let mut h = 900.0f32;
            let mut t = 0.0f64;
            for tok in rest.split_whitespace() {
                if let Some(r) = tok.strip_prefix("rect=") {
                    if let Some((a, b)) = r.split_once('x') {
                        w = a.parse().unwrap_or(w);
                        h = b.parse().unwrap_or(h);
                    }
                } else if let Some(x) = tok.strip_prefix("t=") {
                    t = x.parse().unwrap_or(t);
                }
            }
            frames.push(ReplayFrame { w, h, t, events: Vec::new() });
            continue;
        }
        if let Some(fev) = Fev::parse(line) {
            if let Some(f) = frames.last_mut() {
                f.events.push(fev);
            }
        }
    }
    if frames.is_empty() {
        return None;
    }
    Some(ReplayLog {
        design: design.as_deref().and_then(resolve_design),
        warmup,
        frames,
    })
}

/// Feed the first `nframes` of a replay log through a fresh app.
/// Returns `Some((frame, payload))` on the first panicking frame.
fn replay_frames(
    log: &ReplayLog,
    nframes: usize,
    quiet: bool,
    skip_known: bool,
) -> Option<(usize, String)> {
    let (ctx, mut app) = setup_app();
    let cfg_like = Config {
        seeds: vec![],
        frames: 0,
        deadline_secs: 0,
        logdir: std::path::PathBuf::new(),
        replay: None,
        design: None,
        warmup: log.warmup,
        quiet,
    };
    if let Some(p) = warmup(&ctx, &mut app, &cfg_like, log.design.as_deref()) {
        return Some((0, format!("warmup panic: {p}")));
    }
    let n = nframes.min(log.frames.len());
    for (frame, rf) in log.frames.iter().enumerate().take(n) {
        let mut egui_events = Vec::new();
        for fev in &rf.events {
            egui_events.extend(fev.to_egui());
        }
        let raw = raw_input(rf.w, rf.h, rf.t, egui_events);
        if let Err(p) = pump_frame(&ctx, &mut app, raw) {
            // Push past triaged panics the same way the live fuzzer does, so
            // a replay still reaches a later NEW crash. Regression tests pass
            // skip_known=false so they still fail while their bug is open.
            let payload = panic_payload_str(&p);
            let bt_dbg = take_panic_bt().unwrap_or_default();
            if skip_known && is_known_panic(&payload, &bt_dbg) {
                if !quiet {
                    println!("fuzz: replay skipping known panic at frame {frame}");
                }
                continue;
            }
            return Some((frame, payload));
        }
    }
    None
}

/// Replay a log held in a string. Used by the crash-regression tests below.
#[allow(dead_code)]
fn replay_str(text: &str) -> Option<(usize, String)> {
    let log = parse_replay(text)?;
    replay_frames(&log, log.frames.len(), true, false)
}

/// Binary-search the smallest frame prefix that still reproduces the panic,
/// then truncate the crash file to it. Keeps regression tests small.
fn minimize_crash(path: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let Some(log) = parse_replay(&text) else { return };
    let n = log.frames.len();
    if n <= 40 {
        return;
    }
    println!("fuzz: minimizing {} ({n} frames)...", path.display());
    if replay_frames(&log, n, true, true).is_none() {
        println!("fuzz: full prefix no longer reproduces; keeping original");
        return;
    }
    let mut lo = 1usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        match replay_frames(&log, mid, true, true) {
            Some(_) => hi = mid,
            None => lo = mid + 1,
        }
    }
    println!("fuzz: minimized {n} -> {lo} frames");
    // Truncate: keep header lines + first `lo` frame blocks.
    let mut out: Vec<&str> = Vec::new();
    let mut cur = 0usize;
    for line in text.lines() {
        if line.starts_with("FRAME ") {
            if cur >= lo {
                break;
            }
            cur += 1;
        }
        out.push(line);
    }
    let _ = std::fs::write(path, out.join("\n"));
}

// ---------------------------------------------------------------------------
// Multi-seed driver
// ---------------------------------------------------------------------------

fn run_all(cfg: &Config) {
    sanitize_env();
    install_panic_hook();
    let _ = std::fs::create_dir_all(&cfg.logdir);
    // Per-run anomaly/panic stream.
    let stream = cfg.logdir.join("fuzz-stream.log");
    let f = std::fs::OpenOptions::new().create(true).append(true).open(&stream);
    if let Ok(f) = f {
        let _ = LOG_FILE.set(Mutex::new(f));
    }
    log_line(&format!(
        "RUN seeds={:?} frames={} warmup={} design={:?}",
        cfg.seeds, cfg.frames, cfg.warmup, cfg.design
    ));

    let t0 = Instant::now();
    let mut total_frames = 0usize;
    let mut total_events = 0usize;
    let mut total_panics = 0usize;
    let mut total_anomalies: HashMap<&'static str, usize> = HashMap::new();
    let mut seeds_done = 0usize;
    let mut new_crashes: Vec<std::path::PathBuf> = Vec::new();

    for (i, seed) in cfg.seeds.iter().enumerate() {
        if cfg.deadline_secs > 0 && t0.elapsed().as_secs() >= cfg.deadline_secs {
            say(cfg, "fuzz: deadline reached, stopping");
            break;
        }
        say(cfg, &format!("fuzz: seed {} ({}/{})", seed, i + 1, cfg.seeds.len()));
        let st = run_seed(cfg, *seed);
        total_frames += st.frames;
        total_events += st.events;
        total_panics += st.panics;
        for (k, v) in st.anomalies {
            *total_anomalies.entry(k).or_insert(0) += v;
        }
        new_crashes.extend(st.crashes);
        seeds_done += 1;
        if st.aborted {
            say(cfg, &format!("fuzz: seed {seed} aborted after panic"));
        }
    }

    // Minimize the crash files produced by this run (not old triaged ones).
    new_crashes.sort();
    new_crashes.dedup();
    for c in &new_crashes {
        minimize_crash(c);
    }

    let el = t0.elapsed().as_secs_f32();
    let summary = format!(
        "SUMMARY seeds_done={seeds_done} frames={total_frames} events={total_events} panics={total_panics} anomalies={total_anomalies:?} elapsed={el:.0}s"
    );
    log_line(&summary);
    println!("fuzz {summary}");
}

fn run_replay(cfg: &Config, path: &std::path::Path) {
    sanitize_env();
    install_panic_hook();
    let text = std::fs::read_to_string(path).expect("replay log readable");
    let log = parse_replay(&text).expect("replay log parses");
    println!(
        "fuzz: replaying {} ({} frames, warmup={})",
        path.display(),
        log.frames.len(),
        log.warmup
    );
    match replay_frames(&log, log.frames.len(), cfg.quiet, false) {
        Some((frame, payload)) => println!("fuzz: REPRODUCED panic at frame {frame}: {payload}"),
        None => println!("fuzz: replay clean, no panic"),
    }
}

// ---------------------------------------------------------------------------
// Test entry points
// ---------------------------------------------------------------------------

/// The fuzzer itself. Gated on HELION_FUZZ_RUN=1 so plain `cargo test` stays
/// fast. Run: `HELION_FUZZ_RUN=1 cargo test -p helion-gui --bin helion-ide fuzz_run -- --nocapture`
#[test]
fn fuzz_run() {
    if !env_bool("HELION_FUZZ_RUN", false) {
        println!("fuzz: set HELION_FUZZ_RUN=1 to run the event fuzzer (skipped)");
        return;
    }
    let cfg = Config::from_env();
    match &cfg.replay {
        Some(path) => run_replay(&cfg, path),
        None => run_all(&cfg),
    }
}

/// Regression tests, one per triaged crash replay in fuzz_crashes/.
/// Each replays the exact recorded input sequence and fails if the panic
/// reproduces. While the underlying bug is still open, the test is marked
/// `#[ignore]` (with the reason); remove the attribute once fixed.
#[cfg(test)]
mod crash_regression {
    /// crash-b8e9fc06b1caf963: resizing to a very short window while the
    /// Device canvas is showing panics in `chrome::device_band_share`
    /// (`(after - tables_floor - clock_h - 10.0).clamp(64.0, after)` with
    /// `after < 64.0`; fuzzer seed 11400714819323198485, frame 410,
    /// `EV resize 3840.0 200.0`). Remove the `ignore` once the clamp is fixed;
    /// the test then guards the fix.
    #[test]
    #[ignore = "bug open: device_band_share clamps (64.0, after) with after < 64 on short windows"]
    fn replay_device_band_share_short_window() {
        let text = include_str!("../../../fuzz_crashes/crash-b8e9fc06b1caf963.log");
        match super::replay_str(text) {
            None => {}
            Some((frame, payload)) => panic!(
                "fuzz regression crash-b8e9fc06b1caf963.log reproduced panic at frame {frame}: {payload}"
            ),
        }
    }

    /// crash-87c5907c652b0759: the "More Device" command-palette entry calls
    /// `open_more_workspace(Device)`, whose debug assertion fires because
    /// `apply_more` sets `workspace = Device` but leaves `canvas = Editor`,
    /// so the central pane keeps painting the editor. Fuzzer seed 3,
    /// frame 138. Remove the `ignore` once `apply_more` syncs the canvas;
    /// the test then guards the fix.
    #[test]
    #[ignore = "bug open: apply_more(Device) leaves canvas=Editor, tripping the open_more_workspace assertion"]
    fn replay_more_device_palette_assertion() {
        let text = include_str!("../../../fuzz_crashes/crash-87c5907c652b0759.log");
        match super::replay_str(text) {
            None => {}
            Some((frame, payload)) => panic!(
                "fuzz regression crash-87c5907c652b0759.log reproduced panic at frame {frame}: {payload}"
            ),
        }
    }

    /// crash-59c2055905bbbe0e: same root cause as above, reached through the
    /// flow-navigator sidebar instead of the palette. Fuzzer seed 6,
    /// frame 515. Remove the `ignore` together with the palette-path test.
    #[test]
    #[ignore = "bug open: apply_more(Device) leaves canvas=Editor, tripping the open_more_workspace assertion"]
    fn replay_more_device_flownav_assertion() {
        let text = include_str!("../../../fuzz_crashes/crash-59c2055905bbbe0e.log");
        match super::replay_str(text) {
            None => {}
            Some((frame, payload)) => panic!(
                "fuzz regression crash-59c2055905bbbe0e.log reproduced panic at frame {frame}: {payload}"
            ),
        }
    }
}
