use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics, ShowMessage,
};
use lsp_types::request::{GotoDefinition, HoverRequest, Request as _};
use lsp_types::*;
use serde::Deserialize;
use serde_json::Value;

// --- eXistdb settings ---

#[derive(Clone)]
struct Settings {
    uri: String,
    user: String,
    password: String,
    db_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            uri: "http://localhost:8080/exist".into(),
            user: "admin".into(),
            password: String::new(),
            db_path: String::new(),
        }
    }
}

// --- .existdb.json deserialization ---

#[derive(Deserialize)]
struct ExistConfig {
    servers: HashMap<String, ServerEntry>,
    sync: Option<SyncEntry>,
}

#[derive(Deserialize)]
struct ServerEntry {
    server: String,
    user: String,
    password: String,
    root: Option<String>,
}

#[derive(Deserialize)]
struct SyncEntry {
    server: Option<String>,
    user: Option<String>,
    password: Option<String>,
    root: Option<String>,
}

fn read_workspace_config(workspace: &Path) -> Option<Settings> {
    let config_path = workspace.join(".existdb.json");
    let text = fs::read_to_string(&config_path).ok()?;
    let config: ExistConfig = serde_json::from_str(&text).ok()?;

    if config.servers.is_empty() {
        return None;
    }

    if let Some(sync) = &config.sync {
        if let Some(server_key) = &sync.server {
            if let Some(server) = config.servers.get(server_key) {
                return Some(Settings {
                    uri: server.server.clone(),
                    user: sync.user.clone().unwrap_or_else(|| server.user.clone()),
                    password: sync.password.clone().unwrap_or_else(|| server.password.clone()),
                    db_path: sync
                        .root
                        .clone()
                        .or_else(|| server.root.clone())
                        .unwrap_or_else(|| "/db".into()),
                });
            }
        }
    }

    let (_, server) = config.servers.into_iter().next()?;
    Some(Settings {
        uri: server.server,
        user: server.user,
        password: server.password,
        db_path: server.root.unwrap_or_else(|| "/db".into()),
    })
}

// --- Server startup check & package installation ---

const XAR_VERSION: &str = "1.1.0";
const XAR_FILENAME: &str = "atom-editor-1.1.0.xar";
const XAR_DOWNLOAD_URL: &str =
    "https://raw.githubusercontent.com/wolfgangmm/existdb-langserver/master/resources/atom-editor-1.1.0.xar";
const PACKAGE_URI: &str = "http://exist-db.org/apps/atom-editor";

enum ServerStatus {
    Ok,
    WrongVersion(String),
    NotInstalled,
    Unreachable(String),
}

fn make_agent(connect_secs: u64, total_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(connect_secs))
        .timeout(Duration::from_secs(total_secs))
        .build()
}

fn auth_header(settings: &Settings) -> String {
    format!(
        "Basic {}",
        BASE64.encode(format!("{}:{}", settings.user, settings.password))
    )
}

fn run_xquery(settings: &Settings, agent: &ureq::Agent, xquery: &str) -> Result<Value, String> {
    let url = format!("{}/rest/db", settings.uri);
    agent
        .get(&url)
        .set("Authorization", &auth_header(settings))
        .query("_query", xquery)
        .query("_wrap", "no")
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())
}

fn check_server(settings: &Settings) -> ServerStatus {
    let agent = make_agent(5, 10);
    let xquery = format!(
        r#"xquery version "3.0";
declare namespace expath="http://expath.org/ns/pkg";
declare namespace output="http://www.w3.org/2010/xslt-xquery-serialization";
declare option output:method "json";
declare option output:media-type "application/json";
if ("{pkg}" = repo:list()) then
  let $data := repo:get-resource("{pkg}", "expath-pkg.xml")
  let $xml  := parse-xml(util:binary-to-string($data))
  return
    if ($xml/expath:package/@version = "{ver}") then
      true()
    else
      $xml/expath:package/@version/string()
else
  false()"#,
        pkg = PACKAGE_URI,
        ver = XAR_VERSION,
    );

    match run_xquery(settings, &agent, &xquery) {
        Err(e) => ServerStatus::Unreachable(e),
        Ok(Value::Bool(true)) => ServerStatus::Ok,
        Ok(Value::String(v)) => ServerStatus::WrongVersion(v),
        _ => ServerStatus::NotInstalled,
    }
}

fn download_xar() -> Result<Vec<u8>, String> {
    let agent = make_agent(10, 60);
    let response = agent.get(XAR_DOWNLOAD_URL).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn upload_and_install_xar(settings: &Settings, xar_bytes: &[u8]) -> Result<(), String> {
    let agent = make_agent(10, 30);
    let db_path = format!("/db/system/repo/{}", XAR_FILENAME);
    let upload_url = format!("{}/rest{}", settings.uri, db_path);

    agent
        .put(&upload_url)
        .set("Authorization", &auth_header(settings))
        .set("Content-Type", "application/octet-stream")
        .send_bytes(xar_bytes)
        .map_err(|e| format!("XAR upload failed: {}", e))?;

    let xquery = format!(
        r#"xquery version "3.1";
declare namespace expath="http://expath.org/ns/pkg";
declare namespace output="http://www.w3.org/2010/xslt-xquery-serialization";
declare option output:method "json";
declare option output:media-type "application/json";
declare variable $repo := "http://demo.exist-db.org/exist/apps/public-repo/modules/find.xql";
declare function local:remove($pkg as xs:string) as xs:boolean {{
  if ($pkg = repo:list()) then
    let $u := repo:undeploy($pkg) let $r := repo:remove($pkg) return $r
  else false()
}};
let $xarPath := "{path}"
let $meta    :=
  try {{
    compression:unzip(
      util:binary-doc($xarPath),
      function($p,$t,$x){{ $p="expath-pkg.xml" }}, (),
      function($p,$t,$d,$x){{ $d }}, ()
    )
  }} catch * {{ error(xs:QName("local:err"),"Failed to unpack") }}
let $pkg     := $meta//expath:package/string(@name)
let $_       := local:remove($pkg)
let $_       := repo:install-and-deploy-from-db($xarPath, $repo)
return repo:get-root()"#,
        path = db_path,
    );

    run_xquery(settings, &agent, &xquery).map_err(|e| format!("XAR install failed: {}", e))?;
    Ok(())
}

fn show_message(connection: &Connection, typ: MessageType, message: &str) {
    let notif = Notification::new(
        ShowMessage::METHOD.to_string(),
        ShowMessageParams { typ, message: message.to_string() },
    );
    let _ = connection.sender.send(Message::Notification(notif));
}

/// Runs after LSP initialization: checks eXistdb reachability and whether the
/// atom-editor support package is installed; installs it automatically if not.
fn startup_check(connection: &Connection, settings: &Settings) {
    match check_server(settings) {
        ServerStatus::Ok => {
            // All good — no notification needed.
        }
        ServerStatus::Unreachable(reason) => {
            show_message(
                connection,
                MessageType::WARNING,
                &format!(
                    "XQuery: cannot reach eXistdb at {} — linting and hover will not work. ({})",
                    settings.uri, reason
                ),
            );
        }
        ServerStatus::NotInstalled => {
            show_message(
                connection,
                MessageType::INFO,
                &format!("XQuery: installing eXistdb support package (atom-editor v{})…", XAR_VERSION),
            );
            install_package(connection, settings);
        }
        ServerStatus::WrongVersion(installed) => {
            show_message(
                connection,
                MessageType::INFO,
                &format!(
                    "XQuery: updating eXistdb support package (installed: v{}, required: v{})…",
                    installed, XAR_VERSION
                ),
            );
            install_package(connection, settings);
        }
    }
}

fn install_package(connection: &Connection, settings: &Settings) {
    match download_xar() {
        Err(e) => show_message(
            connection,
            MessageType::ERROR,
            &format!("XQuery: failed to download support package: {}", e),
        ),
        Ok(bytes) => match upload_and_install_xar(settings, &bytes) {
            Ok(()) => show_message(
                connection,
                MessageType::INFO,
                &format!("XQuery: eXistdb support package v{} installed successfully.", XAR_VERSION),
            ),
            Err(e) => show_message(
                connection,
                MessageType::ERROR,
                &format!("XQuery: support package installation failed: {}", e),
            ),
        },
    }
}

// --- Error message parsing ---

fn find_number_after(s: &str, keyword: &str) -> Option<u32> {
    let lower = s.to_lowercase();
    let pos = lower.find(keyword)?;
    let rest = s[pos + keyword.len()..].trim_start_matches(|c: char| c == ':' || c == ' ');
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

struct ParsedError {
    line: u32,
    column: u32,
    message: String,
}

fn parse_error_message(error: &Value) -> Option<ParsedError> {
    let msg = if error.get("line").is_some() {
        error["#text"].as_str()?.to_string()
    } else {
        match error {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    };

    let (line, column) = if let (Some(l), Some(c)) =
        (find_number_after(&msg, "line"), find_number_after(&msg, "column"))
    {
        (l.saturating_sub(1), c.saturating_sub(1))
    } else {
        let l = error["line"]
            .as_str()
            .and_then(|s| s.parse::<u32>().ok())
            .or_else(|| error["line"].as_u64().map(|n| n as u32))
            .unwrap_or(1)
            .saturating_sub(1);
        let c = error["column"]
            .as_str()
            .and_then(|s| s.parse::<u32>().ok())
            .or_else(|| error["column"].as_u64().map(|n| n as u32))
            .unwrap_or(1)
            .saturating_sub(1);
        (l, c)
    };

    Some(ParsedError { line, column, message: msg })
}

// --- Token / cursor helpers ---

fn token_end(text: &str, line: u32, col: u32) -> u32 {
    let line_text = match text.lines().nth(line as usize) {
        Some(l) => l,
        None => return col + 1,
    };
    let chars: Vec<char> = line_text.chars().collect();
    let start = col as usize;
    let end = chars[start..]
        .iter()
        .take_while(|&&c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        .count();
    (start + end.max(1)) as u32
}

// Returns (function_name, arity) when the cursor is on a function-call token.
fn function_at_position(text: &str, line: u32, col: u32) -> Option<(String, usize)> {
    let line_str = text.lines().nth(line as usize)?;
    let chars: Vec<char> = line_str.chars().collect();
    let col = col as usize;

    if col >= chars.len() {
        return None;
    }

    let is_qname = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':');
    if !is_qname(chars[col]) {
        return None;
    }

    let start = chars[..col]
        .iter()
        .rposition(|&c| !is_qname(c))
        .map(|p| p + 1)
        .unwrap_or(0);
    let end = col
        + chars[col..]
            .iter()
            .position(|&c| !is_qname(c))
            .unwrap_or(chars.len() - col);

    let name: String = chars[start..end].iter().collect();
    if name.is_empty() {
        return None;
    }

    let after: String = chars[end..].iter().collect();
    if !after.trim_start().starts_with('(') {
        return None;
    }

    // Byte offset of the `(` in the full document, for arity counting.
    let line_byte_start: usize =
        text.lines().take(line as usize).map(|l| l.len() + 1).sum();
    let paren_char_offset = end + (after.len() - after.trim_start().len());
    let paren_byte = line_byte_start
        + line_str
            .char_indices()
            .nth(paren_char_offset)
            .map(|(b, _)| b)
            .unwrap_or(line_str.len());

    let arity = count_arity(text.as_bytes(), paren_byte);
    Some((name, arity))
}

fn count_arity(bytes: &[u8], offset: usize) -> usize {
    if offset >= bytes.len() || bytes[offset] != b'(' {
        return 0;
    }
    let mut depth = 1usize;
    let mut commas = 0usize;
    let mut has_content = false;

    for &b in &bytes[offset + 1..] {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b',' if depth == 1 => {
                commas += 1;
                has_content = true;
            }
            b' ' | b'\t' | b'\n' | b'\r' => {}
            _ if depth == 1 => has_content = true,
            _ => {}
        }
    }
    if has_content { commas + 1 } else { 0 }
}

// --- Import parsing ---

struct Import {
    prefix: String,
    uri: String,
    source: String,
}

fn parse_imports(text: &str) -> Vec<Import> {
    let re = Regex::new(
        r#"import\s+module\s+namespace\s+(\S+)\s*=\s*["']([^"']+)["']\s+at\s+["']([^"']+)["']"#,
    )
    .unwrap();
    re.captures_iter(text)
        .map(|cap| Import {
            prefix: cap[1].to_string(),
            uri: cap[2].to_string(),
            source: cap[3].to_string(),
        })
        .collect()
}

// --- Local function search ---

// Returns the line number of a `declare function <fn_name>(` in `text`.
fn find_local_function(text: &str, fn_name: &str) -> Option<u32> {
    let escaped = regex::escape(fn_name);
    let re = Regex::new(&format!(
        r"declare\s+(?:%[\w:\-]+(?:\([^)]*\))?\s+)*function\s+{}\s*\(",
        escaped
    ))
    .ok()?;
    let m = re.find(text)?;
    Some(text[..m.start()].chars().filter(|&c| c == '\n').count() as u32)
}

// --- eXistdb autocomplete API (shared by hover and go-to-definition) ---

fn call_autocomplete(
    settings: &Settings,
    fn_name: &str,
    arity: usize,
    imports: &[Import],
    base_path: &str,
) -> Option<Value> {
    let url = format!("{}/apps/atom-editor/atom-autocomplete.xql", settings.uri);
    let signature = format!("{}#{}", fn_name, arity);

    let mut req = ureq::get(&url)
        .set("Authorization", &auth_header(settings))
        .query("signature", &signature)
        .query("base", base_path);

    let prefix = fn_name.split(':').next().unwrap_or("");
    let relevant: Vec<&Import> = if imports.iter().any(|i| i.prefix == prefix) {
        imports.iter().filter(|i| i.prefix == prefix).collect()
    } else {
        imports.iter().collect()
    };
    for imp in &relevant {
        req = req
            .query("mprefix[]", &imp.prefix)
            .query("uri[]", &imp.uri)
            .query("source[]", &imp.source);
    }

    let json: Value = req.call().ok()?.into_json().ok()?;
    let entries = json.as_array()?;
    if entries.is_empty() {
        return None;
    }
    Some(entries[0].clone())
}

fn hover_markdown(desc: &Value) -> Option<String> {
    let mut md = Vec::new();

    let sig = desc["text"].as_str()?;
    if let Some(ret) = desc["leftLabel"].as_str().filter(|s| !s.is_empty()) {
        md.push(format!("**{}** as **{}**", sig, ret));
    } else {
        md.push(format!("**{}**", sig));
    }

    if let Some(doc) = desc["description"].as_str().filter(|s| !s.is_empty()) {
        md.push(doc.to_string());
    }

    if let Some(args) = desc["arguments"].as_array() {
        for arg in args {
            let name = arg["name"].as_str().unwrap_or("");
            let ty = arg["type"].as_str().unwrap_or("");
            let doc = arg["description"].as_str().unwrap_or("");
            if !name.is_empty() {
                md.push(format!("**${}** *{}* {}", name, ty, doc));
            }
        }
    }

    Some(md.join("\n\n"))
}

// --- Path helpers for go-to-definition ---

// Equivalent of Node's path.relative(from, to) for slash-separated db paths.
fn db_relative(from: &str, to: &str) -> PathBuf {
    let from_parts: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();

    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut rel = PathBuf::new();
    for _ in 0..from_parts.len().saturating_sub(common) {
        rel.push("..");
    }
    for part in &to_parts[common..] {
        rel.push(part);
    }
    rel
}

// Given the db path returned by eXistdb, resolve it to a filesystem path.
// Mirrors: path.resolve(path.dirname(currentFsPath), path.relative(currentDbDir, descDbPath))
fn resolve_db_path(
    desc_db_path: &str,
    current_db_dir: &str, // settings.db_path + "/" + rel_dir  (the dir of the current file in the db)
    current_fs_dir: &Path, // filesystem dir of the current open file
) -> Option<PathBuf> {
    let rel = db_relative(current_db_dir, desc_db_path);
    // Canonicalise ".." components manually so we don't need the file to exist yet.
    let mut result = current_fs_dir.to_path_buf();
    for component in rel.components() {
        match component {
            Component::ParentDir => { result.pop(); }
            Component::Normal(s) => result.push(s),
            _ => {}
        }
    }
    Some(result)
}

// --- eXistdb compile call ---

fn lint_document(
    settings: &Settings,
    workspace: Option<&PathBuf>,
    uri: &Url,
    text: &str,
) -> Vec<Diagnostic> {
    let compile_url = format!("{}/apps/atom-editor/compile.xql", settings.uri);

    let rel_path = workspace
        .and_then(|ws| {
            uri.to_file_path().ok().and_then(|file| {
                file.parent()
                    .and_then(|dir| dir.strip_prefix(ws).ok())
                    .map(|rel| rel.to_string_lossy().into_owned())
            })
        })
        .unwrap_or_default();

    let base_path = match (settings.db_path.as_str(), rel_path.as_str()) {
        ("", "") => "/db".to_string(),
        (db, "") => db.to_string(),
        ("", rel) => rel.to_string(),
        (db, rel) => format!("{}/{}", db, rel),
    };

    let result = ureq::put(&compile_url)
        .set("Content-Type", "application/octet-stream")
        .set("X-BasePath", &base_path)
        .set("Authorization", &auth_header(settings))
        .send_bytes(text.as_bytes());

    let response = match result {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    let json: Value = match response.into_json() {
        Ok(j) => j,
        Err(_) => return vec![],
    };

    if json.get("result").and_then(Value::as_str) == Some("pass") {
        return vec![];
    }

    let error = match json.get("error") {
        Some(e) => e,
        None => return vec![],
    };

    match parse_error_message(error) {
        Some(err) => vec![Diagnostic {
            range: Range {
                start: Position { line: err.line, character: err.column },
                end: Position {
                    line: err.line,
                    character: token_end(text, err.line, err.column),
                },
            },
            severity: Some(DiagnosticSeverity::ERROR),
            message: err.message,
            source: Some("xquery".into()),
            ..Default::default()
        }],
        None => vec![],
    }
}

// --- Request handlers ---

fn handle_hover(
    req: &lsp_server::Request,
    settings: &Settings,
    workspace: &Option<PathBuf>,
    documents: &HashMap<String, String>,
) -> Value {
    let params: TextDocumentPositionParams =
        match serde_json::from_value(req.params.clone()) {
            Ok(p) => p,
            Err(_) => return Value::Null,
        };

    let uri = params.text_document.uri;
    let text = match documents.get(uri.as_str()) {
        Some(t) => t,
        None => return Value::Null,
    };

    let (fn_name, arity) =
        match function_at_position(text, params.position.line, params.position.character) {
            Some(f) => f,
            None => return Value::Null,
        };

    let (rel_path, _) = file_rel_paths(&uri, workspace);
    let base_path = make_base_path(&settings.db_path, &rel_path);
    let imports = parse_imports(text);

    let desc = match call_autocomplete(settings, &fn_name, arity, &imports, &base_path) {
        Some(d) => d,
        None => return Value::Null,
    };

    match hover_markdown(&desc) {
        Some(markdown) => serde_json::to_value(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: markdown,
            }),
            range: None,
        })
        .unwrap_or(Value::Null),
        None => Value::Null,
    }
}

fn handle_goto_definition(
    req: &lsp_server::Request,
    settings: &Settings,
    workspace: &Option<PathBuf>,
    documents: &HashMap<String, String>,
) -> Value {
    let params: TextDocumentPositionParams =
        match serde_json::from_value(req.params.clone()) {
            Ok(p) => p,
            Err(_) => return Value::Null,
        };

    let uri = params.text_document.uri.clone();
    let text = match documents.get(uri.as_str()) {
        Some(t) => t,
        None => return Value::Null,
    };

    let (fn_name, arity) =
        match function_at_position(text, params.position.line, params.position.character) {
            Some(f) => f,
            None => return Value::Null,
        };

    // 1. Check local definition first.
    if let Some(def_line) = find_local_function(text, &fn_name) {
        return location_value(uri, def_line);
    }

    // 2. Ask eXistdb for the source file.
    let (rel_path, rel_dir) = file_rel_paths(&uri, workspace);
    let base_path = make_base_path(&settings.db_path, &rel_path);
    let imports = parse_imports(text);

    let desc = match call_autocomplete(settings, &fn_name, arity, &imports, &base_path) {
        Some(d) => d,
        None => return Value::Null,
    };

    let desc_db_path = match desc["path"].as_str().filter(|s| !s.is_empty()) {
        Some(p) => p,
        None => return Value::Null, // built-in or no path info
    };

    // Resolve the db path to a filesystem path.
    let current_db_dir = make_base_path(&settings.db_path, &rel_dir);
    let current_fs_dir = uri
        .to_file_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let fs_path = match current_fs_dir
        .as_deref()
        .and_then(|dir| resolve_db_path(desc_db_path, &current_db_dir, dir))
    {
        Some(p) => p,
        None => return Value::Null,
    };

    // Read the target file and locate the function declaration.
    let target_text = match fs::read_to_string(&fs_path) {
        Ok(t) => t,
        Err(_) => return Value::Null,
    };

    let def_line = find_local_function(&target_text, &fn_name).unwrap_or(0);
    match Url::from_file_path(&fs_path) {
        Ok(target_uri) => location_value(target_uri, def_line),
        Err(_) => Value::Null,
    }
}

// --- Shared utilities ---

// Returns (rel_path_of_file, rel_dir_of_file) as strings relative to the workspace.
fn file_rel_paths(uri: &Url, workspace: &Option<PathBuf>) -> (String, String) {
    let rel_path = workspace
        .as_ref()
        .and_then(|ws| {
            uri.to_file_path().ok().and_then(|file| {
                file.strip_prefix(ws)
                    .ok()
                    .map(|r| r.to_string_lossy().into_owned())
            })
        })
        .unwrap_or_default();

    let rel_dir = workspace
        .as_ref()
        .and_then(|ws| {
            uri.to_file_path().ok().and_then(|file| {
                file.parent()
                    .and_then(|dir| dir.strip_prefix(ws).ok())
                    .map(|r| r.to_string_lossy().into_owned())
            })
        })
        .unwrap_or_default();

    (rel_path, rel_dir)
}

fn make_base_path(db_path: &str, rel: &str) -> String {
    match (db_path, rel) {
        ("", "") => "/db".to_string(),
        (db, "") => db.to_string(),
        ("", r) => r.to_string(),
        (db, r) => format!("{}/{}", db, r),
    }
}

fn location_value(uri: Url, line: u32) -> Value {
    serde_json::to_value(Location {
        uri,
        range: Range {
            start: Position { line, character: 0 },
            end: Position { line, character: 0 },
        },
    })
    .unwrap_or(Value::Null)
}

// --- Main ---

fn main() {
    let (connection, io_threads) = Connection::stdio();

    let (init_id, init_params_raw) = connection.initialize_start().unwrap();
    let init_params: InitializeParams = serde_json::from_value(init_params_raw).unwrap();

    let mut settings = Settings::default();
    if let Some(opts) = &init_params.initialization_options {
        if let Some(s) = opts["server"].as_str() { settings.uri = s.into(); }
        if let Some(s) = opts["user"].as_str() { settings.user = s.into(); }
        if let Some(s) = opts["password"].as_str() { settings.password = s.into(); }
        if let Some(s) = opts["path"].as_str() { settings.db_path = s.into(); }
    }

    let workspace_path: Option<PathBuf> = init_params
        .workspace_folders
        .as_ref()
        .and_then(|f| f.first())
        .and_then(|f| f.uri.to_file_path().ok())
        .or_else(|| {
            #[allow(deprecated)]
            init_params.root_uri.as_ref().and_then(|u| u.to_file_path().ok())
        });

    if let Some(ws) = &workspace_path {
        if let Some(config) = read_workspace_config(ws) {
            settings = config;
        }
    }

    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                ..Default::default()
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        ..Default::default()
    };

    let init_result = serde_json::to_value(InitializeResult {
        capabilities,
        server_info: Some(ServerInfo {
            name: "xquery-language-server".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
        ..Default::default()
    })
    .unwrap();

    connection.initialize_finish(init_id, init_result).unwrap();

    startup_check(&connection, &settings);

    let mut documents: HashMap<String, String> = HashMap::new();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req).unwrap_or(false) {
                    break;
                }
                let result = match req.method.as_str() {
                    HoverRequest::METHOD => {
                        handle_hover(&req, &settings, &workspace_path, &documents)
                    }
                    GotoDefinition::METHOD => {
                        handle_goto_definition(&req, &settings, &workspace_path, &documents)
                    }
                    _ => Value::Null,
                };
                let _ = connection.sender.send(Message::Response(Response::new_ok(req.id, result)));
            }
            Message::Notification(notif) => {
                handle_notification(
                    &connection,
                    &settings,
                    workspace_path.as_ref(),
                    notif,
                    &mut documents,
                );
            }
            Message::Response(_) => {}
        }
    }

    io_threads.join().unwrap();
}

fn publish(connection: &Connection, uri: Url, diagnostics: Vec<Diagnostic>) {
    let params = PublishDiagnosticsParams { uri, diagnostics, version: None };
    let notif = Notification::new(PublishDiagnostics::METHOD.to_string(), params);
    let _ = connection.sender.send(Message::Notification(notif));
}

fn handle_notification(
    connection: &Connection,
    settings: &Settings,
    workspace: Option<&PathBuf>,
    notif: Notification,
    documents: &mut HashMap<String, String>,
) {
    match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            if let Ok(params) =
                serde_json::from_value::<DidOpenTextDocumentParams>(notif.params)
            {
                let uri = params.text_document.uri;
                let text = params.text_document.text;
                let diagnostics = lint_document(settings, workspace, &uri, &text);
                documents.insert(uri.to_string(), text);
                publish(connection, uri, diagnostics);
            }
        }
        DidChangeTextDocument::METHOD => {
            if let Ok(params) =
                serde_json::from_value::<DidChangeTextDocumentParams>(notif.params)
            {
                if let Some(change) = params.content_changes.last() {
                    let uri = params.text_document.uri;
                    let text = change.text.clone();
                    let diagnostics = lint_document(settings, workspace, &uri, &text);
                    documents.insert(uri.to_string(), text);
                    publish(connection, uri, diagnostics);
                }
            }
        }
        DidCloseTextDocument::METHOD => {
            if let Ok(params) =
                serde_json::from_value::<DidCloseTextDocumentParams>(notif.params)
            {
                documents.remove(params.text_document.uri.as_str());
                publish(connection, params.text_document.uri, vec![]);
            }
        }
        _ => {}
    }
}
