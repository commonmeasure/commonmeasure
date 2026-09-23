//! The guide pages are rendered from their Markdown sources, and the
//! rendering keeps the properties a reader depends on: every sidebar entry
//! resolves, every footnote resolves, tables scroll in their own box, the two
//! documents link each other, and the public variant names no product.

use commonmeasure_console::guide::{SOURCES, Variant, console_page, public_pages, render};

fn guide() -> &'static str {
    console_page("context-window-optimisation").expect("the guide renders")
}

fn evidence() -> &'static str {
    console_page("state-of-the-evidence").expect("the companion renders")
}

fn untrusted() -> &'static str {
    console_page("untrusted-context").expect("the untrusted-context guide renders")
}

/// Every `href="#…"` in the sidebar, in order.
fn sidebar_anchors(page: &str) -> Vec<&str> {
    let nav_start = page.find("<nav class=\"toc\"").expect("a sidebar");
    let nav_end = nav_start + page[nav_start..].find("</nav>").expect("sidebar closes");
    page[nav_start..nav_end]
        .split("href=\"#")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap_or_default())
        .collect()
}

#[test]
fn every_sidebar_entry_resolves_to_a_heading_in_the_body() {
    for page in [guide(), evidence(), untrusted()] {
        let anchors = sidebar_anchors(page);
        assert!(anchors.len() > 3, "the sidebar lists the chapters");
        for anchor in anchors {
            assert!(
                page.contains(&format!("id=\"{anchor}\"")),
                "sidebar anchor #{anchor} has no heading"
            );
        }
    }
}

#[test]
fn the_sources_own_in_text_links_still_resolve() {
    let page = guide();
    assert!(page.contains("href=\"#glossary\""));
    assert!(page.contains("id=\"glossary\""));
}

#[test]
fn every_footnote_reference_resolves_and_the_notes_are_one_numbered_list() {
    let page = guide();
    let references = page.matches("<sup id=\"fnref:").count();
    let definitions = page.matches("<li id=\"fn:").count();
    assert!(references > 100, "the guide carries its references");
    assert_eq!(
        definitions,
        SOURCES[0]
            .markdown
            .lines()
            .filter(|l| l.starts_with("[^"))
            .count()
    );
    assert_eq!(page.matches("<section class=\"footnotes\">").count(), 1);
    assert!(page.contains("</ol>\n</section>"));
    for definition in page.split("<li id=\"fn:").skip(1) {
        let label = definition.split('"').next().unwrap_or_default();
        assert!(
            page.contains(&format!("href=\"#fn:{label}\"")),
            "footnote {label} is defined but never referenced"
        );
    }
}

#[test]
fn tables_scroll_inside_their_own_box() {
    for page in [guide(), evidence(), untrusted()] {
        let tables = page.matches("<table>").count();
        assert!(tables > 0);
        assert_eq!(
            page.matches("<div class=\"table-wrap\"><table>").count(),
            tables
        );
    }
}

#[test]
fn the_contents_section_is_replaced_by_the_sidebar() {
    assert!(!guide().contains(">Contents</h2>"));
    assert!(guide().contains("<summary>Contents</summary>"));
}

#[test]
fn the_two_documents_link_each_other_by_console_route() {
    assert!(guide().contains("href=\"/guide/state-of-the-evidence\""));
    assert!(evidence().contains("href=\"/guide\""));
}

#[test]
fn console_pages_use_only_the_local_theme_script() {
    for page in [guide(), evidence(), untrusted()] {
        assert_eq!(page.matches("<script").count(), 1);
        assert!(page.contains("<script src=\"/theme.js\"></script>"));
        assert!(!page.contains("<link "));
        assert_eq!(page.matches("<style>").count(), 1);
        assert!(page.starts_with("<!doctype html>"));
    }
}

#[test]
fn the_masthead_is_derived_from_the_source() {
    let page = guide();
    assert!(page.contains("<b>Reading time</b>about "));
    assert!(page.contains("<b>References</b>"));
    assert!(page.contains("<span class=\"eyebrow\">Common Measure &middot; working guide</span>"));
    assert!(page.contains("<title>Context window optimisation: a working guide</title>"));
}

#[test]
fn the_public_variant_names_no_product_and_links_the_sibling_file() {
    let pages = public_pages().expect("the public build names no product");
    assert_eq!(pages.len(), SOURCES.len());
    for page in &pages {
        let lower = page.html.to_lowercase();
        assert!(!lower.contains("common measure") && !lower.contains("commonmeasure"));
        assert!(
            page.html
                .contains("<span class=\"eyebrow\">Working guide</span>")
        );
    }
    assert!(
        pages[0]
            .html
            .contains("href=\"state-of-the-evidence.html\"")
    );
    assert!(
        pages[1]
            .html
            .contains("href=\"context-window-optimisation.html\"")
    );
}

#[test]
fn a_product_panel_becomes_an_aside_in_the_console_and_is_absent_in_public() {
    let source = commonmeasure_console::guide::Source {
        name: "panel",
        markdown: "# A title\n\n*A standfirst.*\n\n## One\n\nBody.\n\n> **What this means for Common Measure**\n>\n> The panel text.\n\n## Two\n\nMore.\n",
        current_to: "1 January 2026",
        links_checked: false,
    };
    let console = render(&source, Variant::Console);
    assert!(
        console
            .contains("<aside class=\"implication\"><h4>What this means for Common Measure</h4>")
    );
    assert!(console.contains("The panel text."));
    let public = render(&source, Variant::Public);
    assert!(!public.contains("panel text"));
    assert!(!public.to_lowercase().contains("common measure"));
}

#[test]
fn the_untrusted_context_guide_resolves_its_footnotes() {
    let page = untrusted();
    let source = SOURCES
        .iter()
        .find(|source| source.name == "untrusted-context")
        .expect("the source is registered");
    let definitions = page.matches("<li id=\"fn:").count();
    assert!(definitions > 100, "the guide carries its references");
    assert_eq!(
        definitions,
        source
            .markdown
            .lines()
            .filter(|l| l.starts_with("[^"))
            .count()
    );
    for definition in page.split("<li id=\"fn:").skip(1) {
        let label = definition.split('"').next().unwrap_or_default();
        assert!(
            page.contains(&format!("href=\"#fn:{label}\"")),
            "footnote {label} is defined but never referenced"
        );
    }
}

#[test]
fn the_masthead_states_each_guide_s_own_date_and_claims_a_link_check_only_when_run() {
    assert!(guide().contains("<b>Research current to</b>4 August 2026"));
    assert!(guide().contains(", all link-checked</span>"));
    assert!(untrusted().contains("<b>Research current to</b>15 September 2026"));
    assert!(!untrusted().contains(", all link-checked</span>"));
}

#[test]
fn the_public_untrusted_context_page_strips_its_panels() {
    let pages = public_pages().expect("the public build names no product");
    let page = pages
        .iter()
        .find(|page| page.name == "untrusted-context")
        .expect("the public build includes the guide");
    assert!(!page.html.contains("<aside class=\"implication\">"));
}

#[test]
fn explicitly_marked_product_sections_only_appear_in_the_console() {
    let source = commonmeasure_console::guide::Source {
        name: "section",
        markdown: "# Guide\n\nIntroduction.\n\n## Shared\n\nBefore.\n\n<!-- product-only:start -->\n\n## Product\n\nCommon Measure details.\n\n<!-- product-only:end -->\n\n## After\n\nRetained evidence.\n",
        current_to: "15 September 2026",
        links_checked: false,
    };
    let console = render(&source, Variant::Console);
    let public = render(&source, Variant::Public);
    assert!(console.contains("Common Measure details."));
    assert!(!public.contains("Common Measure details."));
    assert!(!public.contains("id=\"product\""));
    assert!(public.contains("Before.") && public.contains("Retained evidence."));
}
