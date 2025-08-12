# XQuery Extension for Zed

A Zed editor extension that provides XQuery language support with syntax highlighting and basic language features.

## Features

- **Syntax Highlighting**: Full XQuery syntax highlighting using Tree-sitter grammar
- **File Type Support**: Automatic recognition of XQuery files (`.xq`, `.xql`, `.xqm`, `.xquery`, `.xqy`)
- **Language Features**:
  - XQuery-specific comment styles (`(:`, `:)`)
  - Smart bracket matching and auto-closing
  - Proper indentation and formatting

## Installation

1. Clone the repository:
   ```
   git clone https://github.com/wolfgangmm/zed-xquery.git
   ```

2. Open the Extensions panel (`Cmd+Shift+X` on macOS)
3. Click `Install Dev Extension`
4. Select the cloned repository (`zed-xquery`) and click Install

## Usage

Once installed, the extension will automatically provide syntax highlighting for XQuery files. Simply open any file with supported extensions (`.xq`, `.xql`, `.xqm`, `.xquery`, `.xqy`) and enjoy enhanced XQuery development experience.

## Acknowledgments

This extension is based on the [Tree-sitter XQuery grammar](https://github.com/grantmacken/tree-sitter-xquery) created by Grant MacKenzie.
