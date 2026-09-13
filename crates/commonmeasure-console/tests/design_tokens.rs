//! The console's contrast gate.
//!
//! The design tokens in `console/styles.css` are shared with the stylesheet
//! of the hub (kept in the hub repository), and each carries a
//! measured WCAG 2.2 obligation (the hub's `docs/design.md` holds the table).
//! The hub enforces them with a Node script in its build; this test enforces
//! the same obligations here, offline, so a token edit that breaks one fails
//! the gate in either repo.

const STYLES: &str = include_str!("../../../console/styles.css");

/// An oklch token value, converted to linear sRGB (clamped into gamut).
#[derive(Clone, Copy)]
struct Rgb {
    r: f64,
    g: f64,
    b: f64,
}

/// OKLab/OKLCH to linear sRGB, per the OKLab reference conversion.
fn oklch_to_rgb(l: f64, c: f64, h_deg: f64) -> Rgb {
    let h = h_deg.to_radians();
    let (a, b) = (c * h.cos(), c * h.sin());
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548_0 * b;
    let (l3, m3, s3) = (l_.powi(3), m_.powi(3), s_.powi(3));
    let clamp = |x: f64| x.clamp(0.0, 1.0);
    Rgb {
        r: clamp(4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3),
        g: clamp(-1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3),
        b: clamp(-0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701_0 * s3),
    }
}

/// WCAG relative luminance from linear sRGB components.
fn luminance(rgb: Rgb) -> f64 {
    0.2126 * rgb.r + 0.7152 * rgb.g + 0.0722 * rgb.b
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Every `--name: oklch(L% C H);` declaration in one stylesheet section,
/// in order of appearance.
fn tokens(section: &str) -> Vec<(String, Rgb)> {
    let mut out = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("--") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_end_matches(';');
        let Some(args) = value
            .strip_prefix("oklch(")
            .and_then(|v| v.strip_suffix(')'))
        else {
            continue;
        };
        let parts: Vec<&str> = args.split_whitespace().collect();
        assert_eq!(parts.len(), 3, "unparsed oklch value on line: {line}");
        let l: f64 = parts[0]
            .strip_suffix('%')
            .expect("oklch lightness written as a percentage")
            .parse()
            .expect("lightness");
        let c: f64 = parts[1].parse().expect("chroma");
        let h: f64 = parts[2].parse().expect("hue");
        out.push((name.trim().to_owned(), oklch_to_rgb(l / 100.0, c, h)));
    }
    out
}

struct Theme {
    name: &'static str,
    values: Vec<(String, Rgb)>,
}

impl Theme {
    fn get(&self, name: &str) -> Rgb {
        self.values
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{} theme is missing --{name}", self.name))
            .1
    }
}

fn themes() -> (Theme, Theme) {
    let (light_part, dark_part) = STYLES
        .split_once("@media (prefers-color-scheme: dark)")
        .expect("styles.css declares a dark theme");
    let light = tokens(light_part);
    assert!(
        !light.is_empty(),
        "no oklch tokens found in the light theme"
    );
    // Dark inherits any token it does not override.
    let mut dark = light.clone();
    for (name, value) in tokens(dark_part) {
        match dark.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 = value,
            None => dark.push((name, value)),
        }
    }
    (
        Theme {
            name: "light",
            values: light,
        },
        Theme {
            name: "dark",
            values: dark,
        },
    )
}

/// The obligations from the shared design language, checked per theme:
/// primary text at AAA on every ground it sits on, secondary text and the
/// state colours at AA including on their own tints, and interactive
/// borders at the 3:1 non-text minimum.
#[test]
fn tokens_meet_their_contrast_obligations() {
    let (light, dark) = themes();
    for theme in [&light, &dark] {
        let check = |fg: &str, bg: &str, floor: f64| {
            let ratio = contrast(theme.get(fg), theme.get(bg));
            assert!(
                ratio >= floor,
                "{}: --{fg} on --{bg} is {ratio:.2}:1, below the {floor}:1 obligation",
                theme.name,
            );
        };
        for ground in ["canvas", "surface", "inset"] {
            check("fg", ground, 7.0);
            check("fg-muted", ground, 4.5);
            check("edge-strong", ground, 3.0);
        }
        check("fg-subtle", "surface", 4.5);
        for text in ["accent", "accent-strong"] {
            check(text, "surface", 4.5);
            check(text, "canvas", 4.5);
        }
        for state in ["ok", "warn", "err"] {
            check(state, "surface", 4.5);
            check(state, &format!("{state}-soft"), 4.5);
            check("fg-muted", &format!("{state}-soft"), 4.5);
        }
    }
}

// ---------------------------------------------------------------------------
// The drift gate. The palette's canonical block lives in the hub's stylesheet
// (`web/src/routes/layout.css` in the hub repository); the console and the guide
// carry hand-copied subsets. Hand-synced copies drift silently, so the copies
// are held the way the vendored htmx build is held: `RECORDED_DIGEST` below is
// the SHA-256 of the canonical palette, and these tests fail loudly when any
// local copy disagrees with it, or the two local copies with each other. The
// digest is over the palette in canonical form: per theme, the `oklch` tokens
// sorted by name, one `light --name: value` line each, whitespace collapsed,
// so the same extraction over the hub's stylesheet, restricted to the names
// declared here, reproduces it. A palette change is made in the hub first,
// then both copies here and this digest in the same change (the failure
// message prints the new digest).

const GUIDE: &str = include_str!("../../../docs/guide/guide.css");

/// SHA-256 of the canonical palette, recorded 2026-09-02 from the hub's
/// `web/src/routes/layout.css`.
const RECORDED_DIGEST: &str = "816cd80ada0f9b959d747ebb71599d83ce259ae31e0bf2dc356ec7dc0db60029";

/// A palette token as written: name, whitespace-collapsed value.
type Declaration = (String, String);

/// Every `--name: oklch(...)` declaration in one stylesheet section as
/// written, whitespace collapsed — the guide packs two declarations on a
/// line, so this splits on `;` rather than trusting line starts.
fn declarations(section: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    for chunk in section.split(';') {
        // A declaration straight after `{` shares its chunk with the
        // selector; only what follows the brace is the declaration.
        let declaration = chunk.rsplit(['{', '}']).next().unwrap_or("").trim();
        let Some(rest) = declaration.strip_prefix("--") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(':') else {
            continue;
        };
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        if value.starts_with("oklch(") {
            out.push((name.trim().to_owned(), value));
        }
    }
    out
}

fn light_and_dark(styles: &str) -> (Vec<Declaration>, Vec<Declaration>) {
    let (light, dark) = styles
        .split_once("@media (prefers-color-scheme: dark)")
        .expect("stylesheet declares a dark theme");
    (declarations(light), declarations(dark))
}

/// The palette in one canonical text form, digestible from any of the
/// copies: per theme, the oklch tokens sorted by name, one per line. The
/// same extraction over the hub's stylesheet — restricted to the names the
/// console declares — must yield the same digest; that is what the record
/// pins.
fn canonical_palette(styles: &str) -> String {
    let (light, dark) = light_and_dark(styles);
    let mut lines = Vec::new();
    for (theme, mut tokens) in [("light", light), ("dark", dark)] {
        tokens.sort();
        for (name, value) in tokens {
            lines.push(format!("{theme} --{name}: {value}"));
        }
    }
    format!("{}\n", lines.join("\n"))
}

/// Catches the seam between the two local copies: every palette token the
/// guide declares must carry the console's value, per theme. The guide is
/// a subset by design (a long read needs fewer roles), never a variant.
#[test]
fn the_guide_palette_is_a_subset_of_the_console_palette() {
    let (console_light, console_dark) = light_and_dark(STYLES);
    let (guide_light, guide_dark) = light_and_dark(GUIDE);
    for (theme, console, guide) in [
        ("light", console_light, guide_light),
        ("dark", console_dark, guide_dark),
    ] {
        assert!(
            !guide.is_empty(),
            "no oklch tokens in the guide's {theme} theme"
        );
        for (name, value) in guide {
            let canonical = console
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("guide.css declares --{name}, unknown to the console"));
            assert_eq!(
                value, canonical.1,
                "{theme}: --{name} differs between guide.css and console/styles.css"
            );
        }
    }
}

/// Catches the seam with the canonical block in the hub: the console's
/// palette must digest to `RECORDED_DIGEST`. Local edits that never went
/// through the hub disagree with the record and fail here.
#[test]
fn the_console_palette_matches_the_recorded_canonical_digest() {
    use sha2::{Digest, Sha256};

    let recorded = RECORDED_DIGEST;
    let palette = canonical_palette(STYLES);
    let digest = format!("{:x}", Sha256::digest(palette.as_bytes()));
    assert_eq!(
        digest, recorded,
        "the console palette no longer matches the recorded canonical digest.\n\
         If the change came through the hub's stylesheet, update both copies\n\
         and record the new digest in RECORDED_DIGEST:\n\
         {digest}\n\ncanonical form:\n{palette}"
    );
}

/// The state colours are three, not four: the old serious/critical split
/// collapsed into err when the palettes converged. A reintroduced fourth
/// level should be a deliberate design decision, not drift.
#[test]
fn state_colours_are_ok_warn_err() {
    for legacy in ["--good", "--warning", "--serious", "--critical"] {
        assert!(
            !STYLES.contains(&format!("{legacy}:")),
            "styles.css declares {legacy}, a token the shared language retired"
        );
    }
}
