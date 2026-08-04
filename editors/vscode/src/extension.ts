import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import {
  ExecuteCommandRequest,
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
let dataDiagnostics: vscode.DiagnosticCollection | undefined;

export async function activate(
  context: vscode.ExtensionContext,
): Promise<void> {
  dataDiagnostics = vscode.languages.createDiagnosticCollection("data-dict (data)");
  context.subscriptions.push(
    dataDiagnostics,
    vscode.commands.registerCommand("dataDict.validateData", () => validateData()),
    vscode.commands.registerCommand("dataDict.restartServer", () =>
      restart(context),
    ),
    // Data diagnostics are a point-in-time snapshot; drop them once the
    // dictionary is edited so stale results don't linger.
    vscode.workspace.onDidChangeTextDocument((e) =>
      dataDiagnostics?.delete(e.document.uri),
    ),
  );
  await start(context);
}

export async function deactivate(): Promise<void> {
  await client?.stop();
  client = undefined;
}

interface DataDiagnostic {
  range: {
    start: { line: number; character: number };
    end: { line: number; character: number };
  };
  severity: number;
  code?: string;
  message: string;
}

async function validateData(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    void vscode.window.showWarningMessage(
      "data-dict: open a data dictionary to validate.",
    );
    return;
  }
  if (!client) {
    void vscode.window.showWarningMessage(
      "data-dict: the language server is not running.",
    );
    return;
  }

  const uri = editor.document.uri;
  try {
    const result = (await client.sendRequest(ExecuteCommandRequest.type, {
      command: "data-dict.validateData",
      arguments: [uri.toString()],
    })) as { summary?: string; diagnostics?: DataDiagnostic[] } | null;

    const diagnostics = (result?.diagnostics ?? []).map(toDiagnostic);
    dataDiagnostics?.set(uri, diagnostics);
    if (result?.summary) {
      void vscode.window.showInformationMessage(`data-dict: ${result.summary}`);
    }
  } catch (err) {
    void vscode.window.showErrorMessage(
      `data-dict: data validation failed. ${String(err)}`,
    );
  }
}

function toDiagnostic(d: DataDiagnostic): vscode.Diagnostic {
  const range = new vscode.Range(
    d.range.start.line,
    d.range.start.character,
    d.range.end.line,
    d.range.end.character,
  );
  const severity =
    d.severity === 2
      ? vscode.DiagnosticSeverity.Warning
      : vscode.DiagnosticSeverity.Error;
  const diagnostic = new vscode.Diagnostic(range, d.message, severity);
  diagnostic.source = "data-dict (data)";
  if (d.code !== undefined) {
    diagnostic.code = d.code;
  }
  return diagnostic;
}

async function start(context: vscode.ExtensionContext): Promise<void> {
  const config = vscode.workspace.getConfiguration("dataDict");
  const command = resolveServerPath(
    config.get<string>("server.path") ?? "data-dict",
    context.extensionPath,
  );
  const globs = config.get<string[]>("files") ?? ["**/data-dict.yaml"];

  // One executable acts as both CLI and language server; `lsp` selects the
  // server, speaking LSP over stdio.
  const serverOptions: ServerOptions = { command, args: ["lsp"] };

  const clientOptions: LanguageClientOptions = {
    documentSelector: globs.map((pattern) => ({ scheme: "file", pattern })),
  };

  client = new LanguageClient(
    "dataDict",
    "data-dict.yaml",
    serverOptions,
    clientOptions,
  );

  try {
    await client.start();
  } catch (err) {
    void vscode.window.showErrorMessage(
      `data-dict language server failed to start (\`${command} lsp\`). ` +
        "Build it with `cargo build -p data-dict-cli --features lsp`, or set " +
        `\`dataDict.server.path\`. ${String(err)}`,
    );
  }
}

async function restart(context: vscode.ExtensionContext): Promise<void> {
  await client?.stop();
  client = undefined;
  await start(context);
}

/// Resolve the server executable. An explicit, non-default setting wins (taken
/// relative to the workspace when not absolute). Otherwise look for a binary
/// built under `target/` — first in the opened workspace, then relative to the
/// extension itself (so running from source via F5 finds this repo's build no
/// matter which project is open) — before falling back to `data-dict` on `PATH`.
function resolveServerPath(configured: string, extensionPath: string): string {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

  if (configured && configured !== "data-dict") {
    if (path.isAbsolute(configured)) {
      return configured;
    }
    return folder ? path.join(folder, configured) : configured;
  }

  // `editors/vscode/` is two levels below the workspace root, where `target/`
  // lives. Check the opened folder first, then the extension's own location.
  const roots = [folder, path.join(extensionPath, "..", "..")].filter(
    (root): root is string => Boolean(root),
  );
  const exe = process.platform === "win32" ? "data-dict.exe" : "data-dict";
  for (const root of roots) {
    for (const profile of ["release", "debug"]) {
      const candidate = path.join(root, "target", profile, exe);
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }

  return "data-dict";
}
