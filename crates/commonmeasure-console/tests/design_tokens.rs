//! Offline contrast and drift checks for the website token release.
//! Generated CSS is embedded; a Rust build needs no Node or network request.

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

/// Every opaque hex or `--name: oklch(L% C H);` declaration in one stylesheet section,
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
        if let Some(hex) = value.strip_prefix('#') {
            assert_eq!(hex.len(), 6, "expected six-digit sRGB token: {line}");
            let channel = |start: usize| {
                let v =
                    f64::from(u8::from_str_radix(&hex[start..start + 2], 16).expect("hex channel"))
                        / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            out.push((
                name.trim().to_owned(),
                Rgb {
                    r: channel(0),
                    g: channel(2),
                    b: channel(4),
                },
            ));
            continue;
        }
        let Some(args) = value
            .strip_prefix("oklch(")
            .and_then(|v| v.strip_suffix(')'))
        else {
            continue;
        };
        if args.contains('/') {
            continue; // Scrims are translucent layers, not text colours.
        }
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
        .split_once(":root[data-theme=\"dark\"]")
        .expect("styles.css declares a dark theme");
    let light = tokens(light_part);
    assert!(
        !light.is_empty(),
        "no colour tokens found in the light theme"
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
        for ground in ["surface", "canvas", "inset"] {
            check("fg-subtle", ground, 4.5);
            assert!(
                contrast(theme.get("action"), theme.get(ground))
                    .max(contrast(theme.get("on-action"), theme.get(ground)))
                    >= 3.0,
                "{}: primary action needs a visible boundary on {ground}",
                theme.name
            );
        }
        check("on-action", "action", 4.5);
        check("on-action", "action-hover", 4.5);
        check("on-accent", "accent", 4.5);
        check("on-accent", "accent-strong", 4.5);
        check("on-brand", "brand", 7.0);
        check("brand-link", "brand", 4.5);
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

const GUIDE: &str = include_str!("../../../docs/guide/guide.css");
const SOURCE: &str = include_str!("../../../design-tokens/tokens.json");

/// A palette token as written: name, whitespace-collapsed value.
type Declaration = (String, String);

/// Every opaque hex or `--name: oklch(...)` declaration in one stylesheet section as
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
        if value.starts_with("oklch(") || value.starts_with('#') {
            out.push((name.trim().to_owned(), value));
        }
    }
    out
}

fn light_and_dark(styles: &str) -> (Vec<Declaration>, Vec<Declaration>) {
    let (light, dark) = styles
        .split_once(":root[data-theme=\"dark\"]")
        .expect("stylesheet declares a dark theme");
    (declarations(light), declarations(dark))
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
            "no colour tokens in the guide's {theme} theme"
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

/// An edited token release without regenerated CSS, or an edited CSS colour
/// without a source change, must fail the ordinary offline Rust test suite.
#[test]
fn generated_palettes_match_the_vendored_website_release() {
    use sha2::{Digest, Sha256};
    let source: serde_json::Value = serde_json::from_str(SOURCE).expect("token release JSON");
    let digest = format!("{:x}", Sha256::digest(SOURCE.as_bytes()));
    for (name, styles) in [("console", STYLES), ("guide", GUIDE)] {
        assert!(
            styles.contains(&format!("source SHA-256 {digest}")),
            "{name}: stale source digest; regenerate design tokens"
        );
        let (light, dark) = light_and_dark(styles);
        for (theme, actual) in [("light", light), ("dark", dark)] {
            for (token, value) in source["themes"][theme].as_object().expect("theme object") {
                let value = value.as_str().expect("CSS value");
                if !(value.starts_with('#') || value.starts_with("oklch(")) {
                    continue;
                }
                let expected = value.split_whitespace().collect::<Vec<_>>().join(" ");
                let values: Vec<_> = actual
                    .iter()
                    .filter(|(key, _)| key == token)
                    .map(|(_, value)| value)
                    .collect();
                assert_eq!(
                    values,
                    vec![&expected],
                    "{name} {theme}: --{token} differs from the website token release"
                );
            }
        }
    }
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
