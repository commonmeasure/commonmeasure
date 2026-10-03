//! Offline drift checks for the vendored website design release. Generated
//! CSS is embedded; a Rust build needs no Node or network request. Contrast
//! is the release's own gate (`design-tokens/generate.mjs` refuses to write a
//! palette that fails an obligation), so these tests check that what is
//! embedded is that release, unchanged.

const STYLES: &str = include_str!("../../../console/styles.css");
const GUIDE: &str = include_str!("../../../docs/guide/guide.css");
const SOURCE: &str = include_str!("../../../design-tokens/tokens.json");
const CHROME: &str = include_str!("../../../design-tokens/chrome.css");

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

/// The light declarations come before the first dark block; the dark ones
/// are declared twice, for the explicit choice and for System under the
/// operating system's dark preference.
fn light_and_dark(styles: &str) -> (Vec<Declaration>, Vec<Declaration>) {
    let (light, dark) = styles
        .split_once("[data-theme='dark']")
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
                assert!(
                    !values.is_empty() && values.iter().all(|v| **v == expected),
                    "{name} {theme}: --{token} differs from the website token release: {values:?}"
                );
            }
        }
    }
}

/// The shared chrome is inlaid by the generator from `chrome.css`; an edit to
/// either copy without the other fails here.
#[test]
fn the_console_carries_the_release_chrome() {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(CHROME.as_bytes()));
    assert!(
        STYLES.contains(&format!("chrome.css SHA-256 {digest}")),
        "console: stale chrome digest; regenerate design tokens"
    );
    let (_, inlaid) = STYLES
        .split_once("@layer components {")
        .expect("styles.css inlays the shared chrome");
    for rule in [
        ".app-shell {",
        ".shell-nav-link[aria-current='page'] {",
        ".theme-choice {",
    ] {
        assert!(inlaid.contains(rule), "chrome rule missing: {rule}");
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
