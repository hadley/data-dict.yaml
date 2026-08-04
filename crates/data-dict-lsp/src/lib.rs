//! Language server for `data-dict.yaml` authoring, so editors can show live
//! diagnostics as a dictionary is written.
//!
//! Every failure reaches us as a [`data_dict::Problem`] already carrying its
//! severity, code, and resolved [`location`](data_dict::Problem::location); this
//! module just maps each one to an LSP diagnostic, keeping the core free of LSP
//! types.

use std::collections::HashMap;

use data_dict::{Level, Problem, Severity, SourceContext};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

/// The source label attached to every diagnostic this server emits.
const SOURCE: &str = "data-dict";

/// The `workspace/executeCommand` that validates a dictionary against the data
/// at each table's `source`.
const VALIDATE_DATA: &str = "data-dict.validateData";

struct Backend {
    client: Client,
    /// Last-seen text of each open document, so a save with no inlined text can
    /// still be re-validated.
    documents: Mutex<HashMap<Url, String>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: SOURCE.to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![VALIDATE_DATA.to_string()],
                    ..Default::default()
                }),
                ..Default::default()
            },
        })
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        if params.command != VALIDATE_DATA {
            return Ok(None);
        }
        let Some(uri) = params
            .arguments
            .first()
            .and_then(Value::as_str)
            .and_then(|s| Url::parse(s).ok())
        else {
            return Ok(None);
        };
        let result = tokio::task::spawn_blocking(move || validate_data_command(&uri))
            .await
            .unwrap_or_else(|_| json!({ "summary": "internal error", "diagnostics": [] }));
        Ok(Some(result))
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        self.documents
            .lock()
            .await
            .insert(doc.uri.clone(), doc.text.clone());
        self.validate_and_publish(doc.uri, doc.text, Some(doc.version))
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // Full-sync: the final change event carries the entire document.
        let Some(change) = params.content_changes.into_iter().next_back() else {
            return;
        };
        let uri = params.text_document.uri;
        self.documents
            .lock()
            .await
            .insert(uri.clone(), change.text.clone());
        self.validate_and_publish(uri, change.text, Some(params.text_document.version))
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = match params.text {
            Some(text) => text,
            None => self
                .documents
                .lock()
                .await
                .get(&uri)
                .cloned()
                .unwrap_or_default(),
        };
        self.validate_and_publish(uri, text, None).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.lock().await.remove(&uri);
        // Clear any squiggles we published for the now-closed document.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }
}

impl Backend {
    async fn validate_and_publish(&self, uri: Url, text: String, version: Option<i32>) {
        let target = uri.clone();
        // Validation is synchronous and CPU-bound; keep it off the LSP event loop.
        let diagnostics = tokio::task::spawn_blocking(move || document_diagnostics(&text, &target))
            .await
            .unwrap_or_default();
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }
}

/// Spec-validate the buffer `content` (no I/O, so it works unsaved) and return
/// its problems as LSP diagnostics. Data comparison is the separate, on-demand
/// [`validate_data_command`].
pub fn document_diagnostics(content: &str, uri: &Url) -> Vec<Diagnostic> {
    let problems = data_dict::validate_spec_str(content, uri.as_str());
    let source = &problems.source;
    problems
        .items
        .iter()
        .map(|p| problem_to_lsp(p, source))
        .collect()
}

fn problem_to_lsp(problem: &Problem, source: &SourceContext) -> Diagnostic {
    Diagnostic {
        range: problem_range(problem, source),
        severity: Some(lsp_severity(problem.severity)),
        code: problem.code.map(|c| NumberOrString::String(c.to_string())),
        source: Some(SOURCE.to_string()),
        message: problem_message(problem),
        ..Default::default()
    }
}

/// A problem's resolved location as an LSP range (both are 0-based). Unlocated
/// problems collapse to the top of the document rather than being dropped.
fn problem_range(problem: &Problem, source: &SourceContext) -> Range {
    match problem.location(source) {
        Some(loc) => Range {
            start: Position {
                line: loc.start_line as u32,
                character: loc.start_column as u32,
            },
            end: Position {
                line: loc.end_line as u32,
                character: loc.end_column as u32,
            },
        },
        None => Range::default(),
    }
}

/// The rule (`expected`), the specifics (`message`), and any `hint`, one per line.
fn problem_message(problem: &Problem) -> String {
    let mut lines = Vec::new();
    if let Some(expected) = &problem.expected {
        lines.push(expected.clone());
    }
    lines.push(problem.message.clone());
    if let Some(hint) = &problem.hint {
        lines.push(hint.clone());
    }
    lines.join("\n")
}

fn lsp_severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    }
}

/// Validate the dictionary at `uri` against each table's parquet `source`,
/// returning `{ summary, diagnostics }`. Reads from disk, so `uri` must be a
/// saved file.
pub fn validate_data_command(uri: &Url) -> Value {
    let Ok(dict_path) = uri.to_file_path() else {
        return json!({ "summary": "Data validation needs a file on disk.", "diagnostics": [] });
    };

    let problems = data_dict::validate_data(&dict_path, None);

    // A spec/pre-flight error means the dictionary must be fixed first. Spec
    // warnings don't block, and every spec problem already shows as a live
    // squiggle, so we surface only the data-facing (meta/data) problems.
    let is_data_facing = |p: &Problem| matches!(p.kind.level(), Some(Level::Meta | Level::Data));
    let dictionary_broken = problems
        .items
        .iter()
        .any(|p| p.severity == Severity::Error && !is_data_facing(p));
    if dictionary_broken {
        return json!({
            "summary": "Fix the dictionary's errors before validating data.",
            "diagnostics": [],
        });
    }

    let source = &problems.source;
    let diagnostics: Vec<Value> = problems
        .items
        .iter()
        .filter(|p| is_data_facing(p))
        .map(|p| data_diagnostic(p, source))
        .collect();

    let summary = if diagnostics.is_empty() {
        "Data matches the dictionary.".to_string()
    } else {
        format!(
            "{} issue(s) found comparing data to the dictionary.",
            diagnostics.len()
        )
    };
    json!({ "summary": summary, "diagnostics": diagnostics })
}

/// Build the JSON diagnostic payload the extension expects for a data problem.
/// `severity` follows the LSP numbering (`1` = error, `2` = warning).
fn data_diagnostic(problem: &Problem, source: &SourceContext) -> Value {
    let range = problem_range(problem, source);
    let severity = match problem.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
    };
    json!({
        "range": {
            "start": { "line": range.start.line, "character": range.start.character },
            "end": { "line": range.end.line, "character": range.end.character },
        },
        "severity": severity,
        "code": problem.code,
        "message": problem_message(problem),
    })
}

/// Run the language server over stdio until the client disconnects. Builds its
/// own Tokio runtime so callers (e.g. the CLI) can stay synchronous.
pub fn run_stdio() -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let (service, socket) = LspService::new(|client| Backend {
            client,
            documents: Mutex::new(HashMap::new()),
        });
        Server::new(stdin, stdout, socket).serve(service).await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnose(content: &str) -> Vec<Diagnostic> {
        let uri = Url::parse("file:///buffer.yaml").unwrap();
        document_diagnostics(content, &uri)
    }

    #[test]
    fn valid_document_has_no_error_diagnostics() {
        // A single-table dictionary still draws an S16 warning, so assert only
        // that a valid document produces no errors.
        let content = "\
$version: 0.1.0
$learn_more: http://data-dict.tidyverse.org/
tables:
  - name: t
    description: A table.
    source:
      parquet: t.parquet
    columns:
      - name: c
        type: string
        examples: [a, b]
        description: A column.
";
        assert!(
            diagnose(content)
                .iter()
                .all(|d| d.severity != Some(DiagnosticSeverity::ERROR))
        );
    }

    #[test]
    fn schema_failure_becomes_error_diagnostic() {
        // Missing the required `$version` key.
        let diagnostics = diagnose("tables: {}\n");
        assert!(!diagnostics.is_empty());
        let d = &diagnostics[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert!(matches!(&d.code, Some(NumberOrString::String(_))));
        assert_eq!(d.source.as_deref(), Some(SOURCE));
    }

    #[test]
    fn spec_failure_carries_code_and_range() {
        // An `enum` column must declare a representation; omitting it is S07.
        let content = "\
$version: 0.1.0
$learn_more: http://data-dict.tidyverse.org/
tables:
  - name: t
    description: A table.
    source:
      parquet: t.parquet
    columns:
      - name: c
        type: enum
        description: A column.
";
        let diagnostics = diagnose(content);
        let s07 = diagnostics
            .iter()
            .find(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "S07"))
            .expect("expected an S07 diagnostic");
        assert_eq!(s07.severity, Some(DiagnosticSeverity::ERROR));
        // The span should point past the document header, not collapse to (0,0).
        assert!(s07.range.start.line > 0);
    }

    #[test]
    fn unparseable_buffer_is_flagged() {
        // A tab in indentation is invalid YAML.
        let diagnostics = diagnose("foo:\n\t- bar\n");
        assert!(!diagnostics.is_empty());
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    fn temp_dir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "data-dict-lsp-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn validate_data_reports_unreadable_source() {
        let dir = temp_dir();
        let dict = dir.join("data-dict.yaml");
        std::fs::write(
            &dict,
            "\
$version: 0.1.0
$learn_more: http://data-dict.tidyverse.org/
tables:
  - name: t
    description: A table.
    source:
      parquet: missing.parquet
    columns:
      - name: c
        type: string
        examples: [a, b]
        description: A column.
",
        )
        .unwrap();

        let uri = Url::from_file_path(&dict).unwrap();
        let result = validate_data_command(&uri);
        let diags = result["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        // M05: the table's `source` points at a file that can't be read.
        assert_eq!(diags[0]["code"], "M05");
        assert_eq!(diags[0]["severity"], 1);
    }

    #[test]
    fn validate_data_refuses_invalid_dictionary() {
        // Missing `$version`: the dictionary itself is invalid, so data
        // validation should decline rather than report data diagnostics.
        let dir = temp_dir();
        let dict = dir.join("data-dict.yaml");
        std::fs::write(&dict, "tables: {}\n").unwrap();
        let uri = Url::from_file_path(&dict).unwrap();
        let result = validate_data_command(&uri);
        assert!(result["diagnostics"].as_array().unwrap().is_empty());
        assert!(
            result["summary"]
                .as_str()
                .unwrap()
                .contains("Fix the dictionary"),
        );
    }
}
