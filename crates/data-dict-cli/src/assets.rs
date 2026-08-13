//! The files the rendered page is stitched from.
//!
//! They are compiled into the binary, so a released `data-dict` is one
//! self-contained executable. [`Assets::Dir`] reads them from a directory
//! instead, which is what lets `render --live` pick up an edit to the page's
//! own CSS or JS without a rebuild.

use std::io;
use std::path::{Path, PathBuf};

/// The page's parts: the template marker each one fills, the file it is
/// written in, and the copy compiled into this binary.
const PARTS: &[(&str, &str, &str)] = &[
    ("{{APP_CSS}}", "app.css", include_str!("../render/app.css")),
    (
        "{{DIAGRAM_CSS}}",
        "diagram.css",
        include_str!("../render/diagram.css"),
    ),
    (
        "{{TABLES_CSS}}",
        "tables.css",
        include_str!("../render/tables.css"),
    ),
    (
        "{{DAGRE_JS}}",
        "dagre.js",
        include_str!("../render/dagre.js"),
    ),
    (
        "{{LAYOUT_JS}}",
        "layout-dagre.js",
        include_str!("../render/layout-dagre.js"),
    ),
    (
        "{{PREACT_JS}}",
        "preact.js",
        include_str!("../render/preact.js"),
    ),
    (
        "{{SHARED_JS}}",
        "shared.js",
        include_str!("../render/shared.js"),
    ),
    (
        "{{COMPONENTS_JS}}",
        "components.js",
        include_str!("../render/components.js"),
    ),
    (
        "{{DIAGRAM_JS}}",
        "diagram.js",
        include_str!("../render/diagram.js"),
    ),
    ("{{APP_JS}}", "app.js", include_str!("../render/app.js")),
];

/// The stylesheet parts of `PARTS`, in the order the template embeds them.
const CSS_FILES: &[&str] = &["app.css", "diagram.css", "tables.css"];

/// The template every part is substituted into.
const PAGE: (&str, &str) = ("index.html", include_str!("../render/index.html"));

/// The live-reload client, added only by `render --live`.
const LIVE_JS: (&str, &str) = ("live.js", include_str!("../render/live.js"));

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
            .chain([PAGE.0, LIVE_JS.0])
            .map(|file| dir.join(file))
            .collect()
    }

    /// The stylesheet files, for `--live` to tell a CSS-only change apart
    /// from one that needs the page rebuilt.
    pub fn css_files(&self) -> Vec<PathBuf> {
        let Assets::Dir(dir) = self else {
            return Vec::new();
        };
        CSS_FILES.iter().map(|file| dir.join(file)).collect()
    }

    /// The page's stylesheet as one document, in template order. Served on
    /// its own by `render --live`, so a CSS edit can be swapped into the
    /// page without a reload.
    pub fn css(&self) -> io::Result<String> {
        let mut css = String::new();
        for (_, file, embedded) in PARTS.iter().filter(|(_, file, _)| CSS_FILES.contains(file)) {
            css.push_str(&self.read(file, embedded)?);
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

    /// Build the page around `dict_json`. `live` adds the reload client, which
    /// only works against the server `--live` runs and so is left out of a
    /// page written to disk.
    ///
    /// The dictionary JSON is substituted last, so a marker spelled out in
    /// someone's prose is embedded as written rather than expanded.
    pub fn render_page(&self, dict_json: &str, live: bool) -> io::Result<String> {
        let mut page = self.read(PAGE.0, PAGE.1)?;
        for (marker, file, embedded) in PARTS {
            page = page.replace(marker, &self.read(file, embedded)?);
        }
        // The marker trails the last script tag, so a page built without the
        // client is byte-for-byte the page that had no marker at all.
        let live_js = if live {
            format!("\n<script>\n{}</script>", self.read(LIVE_JS.0, LIVE_JS.1)?)
        } else {
            String::new()
        };
        Ok(page
            .replace("{{LIVE_JS}}", &live_js)
            .replace("{{DICT_JSON}}", dict_json))
    }
}

/// An export as the JSON embedded in the page. `<` is escaped so nothing in
/// the dictionary can close the page's `<script>` block or open a comment
/// (`</script>`, `<!--`); in JSON, `<` only ever appears inside strings, where
/// `<` spells the same text.
pub fn dict_json(export: &data_dict::Export) -> String {
    serde_json::to_string(export)
        .expect("an export always serializes")
        .replace('<', "\\u003c")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every marker the template spells is one this module knows how to fill,
    /// so adding a part to `index.html` without adding it to `PARTS` fails
    /// here rather than shipping a page with `{{…}}` printed in it.
    #[test]
    fn every_marker_is_substituted() {
        let page = Assets::Embedded.render_page("{}", true).unwrap();
        for marker in PARTS
            .iter()
            .map(|(marker, _, _)| *marker)
            .chain(["{{LIVE_JS}}", "{{DICT_JSON}}"])
        {
            assert!(!page.contains(marker), "{marker} was left in the page");
            assert!(
                PAGE.1.contains(marker),
                "{marker} is filled but the template never asks for it"
            );
        }
        let markers = PAGE.1.matches("{{").count();
        assert_eq!(
            markers,
            PARTS.len() + 2,
            "the template has a marker this module doesn't fill"
        );
    }

    #[test]
    fn live_client_is_added_only_when_live() {
        let embedded = Assets::Embedded;
        assert!(
            !embedded
                .render_page("{}", false)
                .unwrap()
                .contains("EventSource")
        );
        assert!(
            embedded
                .render_page("{}", true)
                .unwrap()
                .contains("EventSource")
        );
    }

    /// The compiled-in copies and the files they were compiled from are the
    /// same page, so `--live` shows what a plain `render` would write.
    #[test]
    fn a_directory_builds_the_same_page_as_the_embedded_copies() {
        let dir = Assets::Dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("render"));
        assert_eq!(
            dir.render_page("{}", true).unwrap(),
            Assets::Embedded.render_page("{}", true).unwrap()
        );
    }

    #[test]
    fn embedded_assets_have_no_files_to_watch() {
        assert!(Assets::Embedded.files().is_empty());
        assert_eq!(
            Assets::Dir(PathBuf::from("x")).files().len(),
            PARTS.len() + 2
        );
    }

    #[test]
    fn css_lists_the_stylesheets_in_template_order() {
        let dir = Assets::Dir(PathBuf::from("x"));
        assert_eq!(
            dir.css_files(),
            CSS_FILES
                .iter()
                .map(|file| PathBuf::from("x").join(file))
                .collect::<Vec<_>>()
        );
        assert!(Assets::Embedded.css_files().is_empty());
    }

    /// The standalone stylesheet is the same CSS the page embeds, so a swap
    /// shows what a reload would.
    #[test]
    fn the_standalone_stylesheet_matches_the_embedded_one() {
        let css = Assets::Embedded.css().unwrap();
        let page = Assets::Embedded.render_page("{}", false).unwrap();
        for file in CSS_FILES {
            let (_, _, embedded) = PARTS.iter().find(|(_, f, _)| f == file).unwrap();
            assert!(css.contains(embedded), "{file} is missing from the css");
            assert!(page.contains(embedded), "{file} is missing from the page");
        }
    }
}
