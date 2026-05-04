# XQuery Extension for Zed

A Zed editor extension that provides XQuery language support with syntax highlighting and eXist-db–backed linting.

## Features

- **Syntax Highlighting**: Full XQuery syntax highlighting via Tree-sitter
- **File Type Support**: `.xq`, `.xql`, `.xqm`, `.xquery`, `.xqy`
- **Linting**: Real-time error diagnostics by compiling against a running eXist-db instance
- **Language Basics**: XQuery comment styles (`(:` / `:)`), bracket matching, auto-closing, indentation

## Requirements

- [Zed](https://zed.dev) editor
- Rust toolchain (`rustup`, `cargo`) — for building the language server
- A running [eXist-db](https://exist-db.org) instance with the [atom-editor support app](https://github.com/eXist-db/atom-existdb) installed

## Installation

### 1. Clone the repository

```sh
git clone https://github.com/wolfgangmm/zed-xquery.git
cd zed-xquery
```

### 2. Build and install the language server

The linting feature is powered by a native language server binary. Build and install it with:

```sh
cargo install --path lsp-server
```

This compiles the server and places the `xquery-lsp-server` binary in `~/.cargo/bin/`, where the extension will find it automatically.

### 3. Install the extension in Zed

1. Open the Extensions panel (`Cmd+Shift+X` on macOS)
2. Click **Install Dev Extension**
3. Select the cloned `zed-xquery` directory and click **Install**

Zed compiles the WASM extension automatically during this step.

### 4. Configure your eXist-db connection

The extension reads connection settings from a `.existdb.json` file in your workspace root (the same format used by the [existdb-langserver](https://github.com/eXist-db/existdb-langserver) VS Code extension):

```json
{
  "servers": {
    "localhost": {
      "server": "http://localhost:8080/exist",
      "user": "admin",
      "password": "",
      "root": "/db/apps/my-app"
    }
  },
  "sync": {
    "server": "localhost"
  }
}
```

If no `.existdb.json` is present, the extension falls back to defaults (`http://localhost:8080/exist`, user `admin`, empty password). You can also override the connection via Zed's `settings.json`:

```json
{
  "lsp": {
    "xquery-lsp": {
      "initialization_options": {
        "server": "http://localhost:8080/exist",
        "user": "admin",
        "password": "",
        "path": "/db/apps/my-app"
      }
    }
  }
}
```

## Updating the language server

After pulling new changes, rebuild and reinstall the binary:

```sh
cargo install --path lsp-server
```

Then use **Rebuild Extension** in Zed's Extensions panel to reload the WASM extension.

## Acknowledgments

- Tree-sitter grammar by [Grant MacKenzie](https://github.com/grantmacken/tree-sitter-xquery)
- Linting via the [atom-editor support app](https://github.com/eXist-db/atom-existdb) for eXist-db, as used by [existdb-langserver](https://github.com/eXist-db/existdb-langserver)
