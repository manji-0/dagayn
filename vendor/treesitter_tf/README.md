# tree-sitter-terraform

A [tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for [Terraform](https://www.terraform.io/) (`.tf` and `.tfvars` files).

## Overview

This grammar provides **Terraform-specific node types** for each top-level block kind (`resource_block`, `data_block`, `variable_block`, etc.), unlike [tree-sitter-hcl](https://github.com/tree-sitter-grammars/tree-sitter-hcl) which exposes a generic `block` node. Having dedicated node types allows tools to instantly determine the block kind without inspecting child nodes — particularly useful for structural analysis tools, code indexers, and language servers.

### Supported block types

| Block keyword | Node type | Labels |
|--------------|-----------|--------|
| `resource` | `resource_block` | `type`, `name` |
| `data` | `data_block` | `type`, `name` |
| `variable` | `variable_block` | `name` |
| `output` | `output_block` | `name` |
| `module` | `module_block` | `name` |
| `provider` | `provider_block` | `name` |
| `locals` | `locals_block` | — |
| `terraform` | `terraform_block` | — |
| `moved` | `moved_block` | — |
| `import` | `import_block` | — |
| `check` | `check_block` | `name` |
| `removed` | `removed_block` | — |
| `ephemeral` | `ephemeral_block` | `type`, `name` |

## Supported Syntax

- **Top-level blocks** — all 13 Terraform block types listed above
- **Nested blocks** — `lifecycle`, `dynamic`, `content`, custom provider blocks, etc.
- **Attributes** — `name = expression`
- **Expressions** — literals, variable references, attribute access, index access, function calls, unary/binary operators, ternary conditionals, for expressions, splat operators, collection literals
- **String templates** — `"text ${interpolation} more"` with full nesting support
- **Template directives** — `%{if cond}...%{else}...%{endif}` and `%{for x in list}...%{endfor}`
- **Heredocs** — `<<EOF...EOF` and indented `<<-EOF...EOF`
- **Comments** — `#`, `//`, `/* ... */`
- **Type constraints** — `string`, `number`, `bool`, `list(T)`, `map(T)`, `set(T)`, `object({...})`, `tuple([...])`, `any`

## Installation

### Using tree-sitter CLI

```bash
npm install tree-sitter-terraform
```

### Using Cargo (Rust)

```toml
[dependencies]
tree-sitter-terraform = "0.0.1"
```

### From source

```bash
git clone https://github.com/manji-0/tree-sitter-terraform.git
cd tree-sitter-terraform
npm install
npm run generate
```

## Usage

### CLI

```bash
# Parse a Terraform file
tree-sitter parse main.tf

# Run syntax highlighting query
tree-sitter query queries/highlights.scm main.tf
```

### Node.js

```js
const Parser = require('tree-sitter');
const Terraform = require('tree-sitter-terraform');

const parser = new Parser();
parser.setLanguage(Terraform);

const tree = parser.parse(`
resource "aws_s3_bucket" "example" {
  bucket = "my-bucket"
}
`);

const root = tree.rootNode;
const resourceBlock = root.child(0);
console.log(resourceBlock.type);          // "resource_block"
console.log(resourceBlock.childForFieldName('type').text);  // '"aws_s3_bucket"'
console.log(resourceBlock.childForFieldName('name').text);  // '"example"'
```

### Rust

```rust
use tree_sitter::Parser;

fn main() {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_terraform::LANGUAGE.into()).unwrap();

    let source = r#"
resource "aws_s3_bucket" "example" {
  bucket = "my-bucket"
}
"#;
    let tree = parser.parse(source, None).unwrap();
    println!("{}", tree.root_node().to_sexp());
}
```

## Editor Integration

### Neovim (nvim-treesitter)

Add to your nvim-treesitter config:

```lua
local parser_config = require('nvim-treesitter.parsers').get_parser_configs()
parser_config.terraform = {
  install_info = {
    url = 'https://github.com/manji-0/tree-sitter-terraform',
    files = { 'src/parser.c', 'src/scanner.c' },
    branch = 'main',
  },
  filetype = 'terraform',
}
vim.treesitter.language.register('terraform', { 'tf', 'tfvars' })
```

### Helix

Copy the query files to `~/.config/helix/runtime/queries/terraform/`:

```bash
mkdir -p ~/.config/helix/runtime/queries/terraform
cp queries/highlights.scm ~/.config/helix/runtime/queries/terraform/
cp queries/tags.scm        ~/.config/helix/runtime/queries/terraform/
cp queries/indents.scm     ~/.config/helix/runtime/queries/terraform/
```

### Zed

The `tree-sitter.json` file provides the required metadata for Zed extensions.

## Query Files

| File | Purpose |
|------|---------|
| `queries/highlights.scm` | Syntax highlighting — keywords, operators, literals, strings, identifiers, built-in variable prefixes (`var`, `local`, `each`, `count`, …) |
| `queries/tags.scm` | Symbol definitions for code navigation — `resource`, `data`, `variable`, `output`, `module`, `locals`, `provider`, `ephemeral` blocks |
| `queries/folds.scm` | Code folding — `block_body`, `tuple`, `object`, `template_for`, `template_if` |
| `queries/indents.scm` | Editor indentation — block bodies, tuples, objects, function call arguments |

## Development

### Prerequisites

Install [devbox](https://www.jetify.com/devbox), then:

```bash
devbox shell   # enters environment with tree-sitter 0.25.6 and Node.js 22
```

### Commands

```bash
# Regenerate the parser from grammar.js
devbox run generate
# equivalent: devbox run -- tree-sitter generate

# Run all corpus tests (80 test cases)
devbox run test
# equivalent: devbox run -- tree-sitter test

# Parse a file and print the syntax tree
devbox run -- tree-sitter parse examples/basic.tf

# Run a query against a file
devbox run -- tree-sitter query queries/highlights.scm examples/basic.tf
```

### Project structure

```
grammar.js          # Grammar definition (core)
src/
  parser.c          # Generated parser (do not edit)
  scanner.c         # External scanner for string templates and heredocs
queries/
  highlights.scm    # Syntax highlighting
  tags.scm          # Symbol definitions
  folds.scm         # Code folding
  indents.scm       # Indentation
test/corpus/        # Tree-sitter test corpus (16 files, 80 cases)
examples/
  basic.tf          # Basic usage example
  advanced.tf       # Advanced patterns (dynamic blocks, for_each, check, moved, …)
```

## License

MIT — see [LICENSE](LICENSE).
