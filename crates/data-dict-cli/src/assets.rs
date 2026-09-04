//! The files the rendered pages are stitched from: the dictionary page `render`
//! writes, and the validation report page `validate-* --html` writes.
//!
//! They are compiled into the binary, so a released `data-dict` is one
//! self-contained executable. [`Assets::Dir`] reads them from a directory
//! instead, which is what lets `render --live` pick up an edit to the page's
//! own CSS or JS without a rebuild.
//!
//! The files live in three directories under `render/`: `shared/` for what
//! both pages carry, `dict/` for the dictionary page only, and `report/` for
//! the validation report only. Both pages draw from one [`PARTS`] table, so
//! a shared file is compiled in once; a [`Page`] names the subset it embeds.

use std::io;
use std::path::{Path, PathBuf};

/// The page's parts: the template marker each one fills, the file it is
/// written in (relative to `render/`), and the copy compiled into this binary.
const PARTS: &[(&str, &str, &str)] = &[
    (
        "{{APP_CSS}}",
        "shared/app.css",
        include_str!("../render/shared/app.css"),
    ),
    (
        "{{DIAGRAM_CSS}}",
        "dict/diagram.css",
        include_str!("../render/dict/diagram.css"),
    ),
    (
        "{{TABLES_CSS}}",
        "shared/tables.css",
        include_str!("../render/shared/tables.css"),
    ),
    (
        "{{DAGRE_JS}}",
        "dict/dagre.js",
        include_str!("../render/dict/dagre.js"),
    ),
    (
        "{{LAYOUT_JS}}",
        "dict/layout-dagre.js",
        include_str!("../render/dict/layout-dagre.js"),
    ),
    (
        "{{PREACT_JS}}",
        "shared/preact.js",
        include_str!("../render/shared/preact.js"),
    ),
    (
        "{{SHARED_JS}}",
        "shared/shared.js",
        include_str!("../render/shared/shared.js"),
    ),
    (
        "{{ROUTE_JS}}",
        "shared/route.js",
        include_str!("../render/shared/route.js"),
    ),
    (
        "{{DICT_JS}}",
        "dict/dict.js",
        include_str!("../render/dict/dict.js"),
    ),
    (
        "{{COMPONENTS_JS}}",
        "shared/components.js",
        include_str!("../render/shared/components.js"),
    ),
    (
        "{{PROSE_JS}}",
        "dict/prose.js",
        include_str!("../render/dict/prose.js"),
    ),
    (
        "{{DIAGRAM_JS}}",
        "dict/diagram.js",
        include_str!("../render/dict/diagram.js"),
    ),
    (
        "{{APP_JS}}",
        "dict/app.js",
        include_str!("../render/dict/app.js"),
    ),
    (
        "{{DIAGNOSTIC_JS}}",
        "report/diagnostic.js",
        include_str!("../render/report/diagnostic.js"),
    ),
    (
        "{{YAML_EXCERPT_JS}}",
        "report/yaml-excerpt.js",
        include_str!("../render/report/yaml-excerpt.js"),
    ),
    (
        "{{SUGGESTION_JS}}",
        "report/suggestion.js",
        include_str!("../render/report/suggestion.js"),
    ),
    (
        "{{ROWS_JS}}",
        "report/rows.js",
        include_str!("../render/report/rows.js"),
    ),
    (
        "{{REPORT_CSS}}",
        "report/report.css",
        include_str!("../render/report/report.css"),
    ),
    (
        "{{STEPS_JS}}",
        "report/steps.js",
        include_str!("../render/report/steps.js"),
    ),
    (
        "{{PROBLEMS_JS}}",
        "report/problems.js",
        include_str!("../render/report/problems.js"),
    ),
    (
        "{{REPORT_JS}}",
        "report/report.js",
        include_str!("../render/report/report.js"),
    ),
];

/// The marker and compiled-in copy of one part, by file name.
fn part(file: &str) -> (&'static str, &'static str) {
    PARTS
        .iter()
        .find(|(_, name, _)| *name == file)
        .map(|(marker, _, embedded)| (*marker, *embedded))
        .expect("every page part is in PARTS")
}

/// One page: its template, the parts it embeds, and the documents a build fills
/// it with.
pub(crate) struct Page {
    file: &'static str,
    embedded: &'static str,
    /// The `PARTS` files this page embeds, in the order the template does.
    parts: &'static [&'static str],
    /// The markers a build fills with a JSON document, each substituted last so
    /// a marker spelled inside one is embedded as written.
    docs: &'static [&'static str],
    /// Whether `--live` serves this page, which adds the reload client.
    live: bool,
}

impl Page {
    /// The stylesheet parts, in the order the template embeds them.
    fn css(&self) -> impl Iterator<Item = &'static str> {
        self.parts.iter().copied().filter(|f| f.ends_with(".css"))
    }
}

/// The dictionary page, written by `render`.
const DICT_PAGE: Page = Page {
    file: "dict/index.html",
    embedded: include_str!("../render/dict/index.html"),
    parts: &[
        "shared/app.css",
        "dict/diagram.css",
        "shared/tables.css",
        "dict/dagre.js",
        "dict/layout-dagre.js",
        "shared/preact.js",
        "shared/shared.js",
        "shared/route.js",
        "dict/dict.js",
        "shared/components.js",
        "dict/prose.js",
        "dict/diagram.js",
        "dict/app.js",
    ],
    docs: &["{{DICT_JSON}}"],
    live: true,
};

/// The validation report page, written by `validate-* --html`. It carries no
/// relationship diagram, so it leaves out the layout engine the dictionary page
/// needs.
const REPORT_PAGE: Page = Page {
    file: "report/report.html",
    embedded: include_str!("../render/report/report.html"),
    parts: &[
        "shared/app.css",
        "shared/tables.css",
        "report/report.css",
        "shared/preact.js",
        "shared/shared.js",
        "shared/route.js",
        "shared/components.js",
        "report/diagnostic.js",
        "report/yaml-excerpt.js",
        "report/suggestion.js",
        "report/rows.js",
        "report/steps.js",
        "report/problems.js",
        "report/report.js",
    ],
    docs: &["{{REPORT_JSON}}", "{{SOURCE_JSON}}", "{{CHECKS_JSON}}"],
    live: true,
};

/// The page each `--live` mode serves. `Report` carries the run's arguments:
/// the level to validate at and the table to restrict to, if any.
pub(crate) enum LivePage {
    Dict,
    Report {
        level: data_dict::Level,
        table: Option<String>,
    },
}

impl LivePage {
    pub(crate) fn page(&self) -> &'static Page {
        match self {
            LivePage::Dict => &DICT_PAGE,
            LivePage::Report { .. } => &REPORT_PAGE,
        }
    }
}

const PAGES: &[&Page] = &[&DICT_PAGE, &REPORT_PAGE];

/// The live-reload client, added only by `render --live`.
const LIVE_JS: (&str, &str) = ("shared/live.js", include_str!("../render/shared/live.js"));

/// Where the page's parts are read from.
pub enum Assets {
    /// The copies compiled into this binary.
    Embedded,
    /// A directory holding the same file names, re-read on every build.
    Dir(PathBuf),
}

impl Assets {
    /// The page's parts as a directory, when running a development build from
    /// the source tree; otherwise the compiled-in copies. Lets `--live` reload
    /// on an edit to the page itself without a `cargo build` in between.
    pub fn detect() -> Self {
        if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR").filter(|_| cfg!(debug_assertions))
        {
            let dir = Path::new(manifest).join("render");
            if dir.is_dir() {
                return Assets::Dir(dir);
            }
        }
        Assets::Embedded
    }

    /// Every file a build reads, for `--live` to watch. Empty for
    /// [`Assets::Embedded`], whose parts can't change while the process runs.
    pub fn files(&self) -> Vec<PathBuf> {
        let Assets::Dir(dir) = self else {
            return Vec::new();
        };
        PARTS
            .iter()
            .map(|(_, file, _)| *file)
            .chain(PAGES.iter().map(|page| page.file))
            .chain([LIVE_JS.0])
            .map(|file| dir.join(file))
            .collect()
    }

    /// The stylesheet files of the page `--live` serves, for it to tell a
    /// CSS-only change apart from one that needs the page rebuilt.
    pub fn css_files(&self, page: &LivePage) -> Vec<PathBuf> {
        let Assets::Dir(dir) = self else {
            return Vec::new();
        };
        page.page().css().map(|file| dir.join(file)).collect()
    }

    /// The stylesheet of the page `--live` serves, as one document in template
    /// order. Served on its own so a CSS edit can be swapped into the page
    /// without a reload.
    pub fn css(&self, page: &LivePage) -> io::Result<String> {
        let mut css = String::new();
        for file in page.page().css() {
            css.push_str(&self.read(file, part(file).1)?);
            css.push('\n');
        }
        Ok(css)
    }

    fn read(&self, file: &str, embedded: &'static str) -> io::Result<String> {
        match self {
            Assets::Embedded => Ok(embedded.to_string()),
            Assets::Dir(dir) => std::fs::read_to_string(dir.join(file)),
        }
    }

    /// Build the dictionary page around `dict_json`. `live` adds the reload
    /// client, which only works against the server `--live` runs and so is left
    /// out of a page written to disk.
    pub fn render_dict_page(&self, dict_json: &str, live: bool) -> io::Result<String> {
        self.build(&DICT_PAGE, &[dict_json], live)
    }

    /// Build the validation report page around a report and the dictionary text
    /// its spans are measured against, both already escaped for embedding. The
    /// page carries the check catalogue too, so it can name a code offline.
    /// `live` adds the reload client, as it does for the dictionary page.
    pub fn render_report_page(
        &self,
        report_json: &str,
        source_json: &str,
        live: bool,
    ) -> io::Result<String> {
        let checks = embed_json(&data_dict::checks());
        self.build(&REPORT_PAGE, &[report_json, source_json, &checks], live)
    }

    /// `docs` are the page's documents in the order [`Page::docs`] names their
    /// markers.
    fn build(&self, page: &Page, docs: &[&str], live: bool) -> io::Result<String> {
        assert_eq!(page.docs.len(), docs.len(), "{} document count", page.file);
        let mut html = self.read(page.file, page.embedded)?;
        for file in page.parts {
            let (marker, embedded) = part(file);
            html = html.replace(marker, &self.read(file, embedded)?);
        }
        if page.live {
            // The marker trails the last script tag, so a page built without the
            // client is byte-for-byte the page that had no marker at all.
            let live_js = if live {
                format!("\n<script>\n{}</script>", self.read(LIVE_JS.0, LIVE_JS.1)?)
            } else {
                String::new()
            };
            html = html.replace("{{LIVE_JS}}", &live_js);
        }
        let filled: Vec<(&str, &str)> = page
            .docs
            .iter()
            .copied()
            .zip(docs.iter().copied())
            .collect();
        Ok(fill_documents(&html, &filled))
    }
}

/// Substitute each document into the page in one pass, never rescanning what it
/// just wrote. A document holds text nobody here controls — a dictionary's prose
/// or its literal YAML — so it can spell a marker, its own or another document's,
/// and must be embedded as written rather than expanded.
fn fill_documents(page: &str, docs: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(page.len());
    let mut rest = page;
    loop {
        let next = docs
            .iter()
            .filter_map(|(marker, doc)| rest.find(marker).map(|at| (at, *marker, *doc)))
            .min_by_key(|(at, ..)| *at);
        let Some((at, marker, doc)) = next else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at]);
        out.push_str(doc);
        rest = &rest[at + marker.len()..];
    }
}

/// A JSON document as the page embeds it. `<` is escaped so nothing in the
/// document can close the page's `<script>` block or open a comment
/// (`</script>`, `<!--`); in JSON, `<` only ever appears inside strings, where
/// `<` spells the same text.
pub fn escape_embedded(json: &str) -> String {
    json.replace('<', "\\u003c")
}

/// Serialize a value as the JSON document the page embeds. A `&str` becomes a
/// JSON string, which is how the dictionary's own text rides along beside the
/// report that locates spans in it.
pub fn embed_json(value: &impl serde::Serialize) -> String {
    escape_embedded(&serde_json::to_string(value).expect("an embedded document always serializes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every marker a build of `page` fills, so a template that spells one this
    /// module doesn't know about can be caught.
    fn markers(page: &Page) -> impl Iterator<Item = &'static str> {
        page.parts
            .iter()
            .map(|file| part(file).0)
            .chain(page.docs.iter().copied())
            .chain(page.live.then_some("{{LIVE_JS}}"))
    }

    /// Build one page through its public entry point, with every document set to
    /// `{}`, for the tests that only care about the parts around them.
    fn build(assets: &Assets, page: &Page, live: bool) -> String {
        if page.file == REPORT_PAGE.file {
            assets.render_report_page("{}", "{}", live).unwrap()
        } else {
            assets.render_dict_page("{}", live).unwrap()
        }
    }

    /// Every marker a template spells is one this module knows how to fill, so
    /// adding a part to a template without registering it fails here rather
    /// than shipping a page with `{{…}}` printed in it.
    #[test]
    fn every_marker_is_substituted() {
        for page in PAGES {
            let built = build(&Assets::Embedded, page, true);
            for marker in markers(page) {
                assert!(
                    !built.contains(marker),
                    "{marker} was left in {}",
                    page.file
                );
                assert!(
                    page.embedded.contains(marker),
                    "{marker} is filled but {} never asks for it",
                    page.file
                );
            }
            assert_eq!(
                page.embedded.matches("{{").count(),
                markers(page).count(),
                "{} has a marker this module doesn't fill",
                page.file
            );
        }
    }

    #[test]
    fn live_client_is_added_only_when_live() {
        let embedded = Assets::Embedded;
        assert!(!build(&embedded, &DICT_PAGE, false).contains("EventSource"));
        assert!(build(&embedded, &DICT_PAGE, true).contains("EventSource"));
    }

    /// The report page carries the live client only when `--live` serves it,
    /// like the dictionary page.
    #[test]
    fn the_report_page_carries_the_live_client_only_when_live() {
        assert!(!build(&Assets::Embedded, &REPORT_PAGE, false).contains("EventSource"));
        assert!(build(&Assets::Embedded, &REPORT_PAGE, true).contains("EventSource"));
    }

    /// The report page has no relationship diagram, so it leaves the layout
    /// engine out rather than shipping it unused in every report.
    #[test]
    fn the_report_page_leaves_out_the_diagram() {
        let built = build(&Assets::Embedded, &REPORT_PAGE, false);
        for absent in ["dagre", "DIAGRAM_INIT"] {
            assert!(!built.contains(absent), "{absent} is in the report page");
        }
    }

    /// The compiled-in copies and the files they were compiled from are the
    /// same pages, so `--live` shows what a plain `render` would write.
    #[test]
    fn a_directory_builds_the_same_page_as_the_embedded_copies() {
        let dir = Assets::Dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("render"));
        for page in PAGES {
            assert_eq!(
                build(&dir, page, true),
                build(&Assets::Embedded, page, true),
                "{}",
                page.file
            );
        }
    }

    #[test]
    fn embedded_assets_have_no_files_to_watch() {
        assert!(Assets::Embedded.files().is_empty());
        assert_eq!(
            Assets::Dir(PathBuf::from("x")).files().len(),
            PARTS.len() + PAGES.len() + 1
        );
    }

    #[test]
    fn css_lists_the_stylesheets_in_template_order() {
        let dir = Assets::Dir(PathBuf::from("x"));
        assert_eq!(
            dir.css_files(&LivePage::Dict),
            ["shared/app.css", "dict/diagram.css", "shared/tables.css"]
                .iter()
                .map(|file| PathBuf::from("x").join(file))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            dir.css_files(&LivePage::Report {
                level: data_dict::Level::Data,
                table: None,
            }),
            ["shared/app.css", "shared/tables.css", "report/report.css"]
                .iter()
                .map(|file| PathBuf::from("x").join(file))
                .collect::<Vec<_>>()
        );
        assert!(Assets::Embedded.css_files(&LivePage::Dict).is_empty());
    }

    /// The standalone stylesheet is the same CSS the page embeds, so a swap
    /// shows what a reload would.
    #[test]
    fn the_standalone_stylesheet_matches_the_embedded_one() {
        let css = Assets::Embedded.css(&LivePage::Dict).unwrap();
        let page = build(&Assets::Embedded, &DICT_PAGE, false);
        for file in DICT_PAGE.css() {
            let embedded = part(file).1;
            assert!(css.contains(embedded), "{file} is missing from the css");
            assert!(page.contains(embedded), "{file} is missing from the page");
        }
    }

    /// A document is text nobody here controls, so one that spells a marker —
    /// its own, or another document's — is embedded as written.
    #[test]
    fn a_document_that_spells_a_marker_is_embedded_as_written() {
        let filled = fill_documents(
            "a{{ONE}}b{{TWO}}c",
            &[("{{ONE}}", "{{TWO}}"), ("{{TWO}}", "2")],
        );
        assert_eq!(filled, "a{{TWO}}b2c");
    }

    /// Nothing in a document can close the page's `<script>` block.
    #[test]
    fn a_document_cannot_close_the_script_block() {
        let escaped = embed_json(&"</script><script>alert(1)</script>");
        assert!(escaped.contains("\\u003c/script>"), "{escaped}");
        assert!(!escaped.contains("</script>"), "{escaped}");
    }
}
