; Block keywords
["resource" "data" "variable" "output" "module" "provider"
 "locals" "terraform" "moved" "import" "check"
 "removed" "ephemeral"] @keyword

; Template directives
["for" "endfor" "in" "if" "else" "endif"] @keyword.control

; Operators
["!" "*" "/" "%" "+" "-" ">" ">=" "<" "<=" "==" "!=" "&&" "||"] @operator
["?" ":" "=>"] @operator

; Punctuation
["{" "}" "[" "]" "(" ")"] @punctuation.bracket
["." ","] @punctuation.delimiter

; Literals
(numeric_lit) @number
(bool_lit) @boolean
(null_lit) @constant.builtin

; Comments
(comment) @comment

; Strings / templates
(string_lit) @string
(quoted_template) @string
(heredoc_template) @string
(heredoc_identifier) @label

; Interpolation marker (strip ~)
(strip_marker) @punctuation.special

; Identifiers
(identifier) @variable
(function_call (function_name) @function.call)
(attribute name: (identifier) @variable.member)

; Well-known variable prefixes
((variable_expr (identifier) @variable.builtin)
  (#any-of? @variable.builtin "var" "local" "module" "data" "path" "self" "each" "count"))

; Block types
(resource_block type: (string_lit) @type)
(resource_block name: (string_lit) @string.special)
(data_block type: (string_lit) @type)
(data_block name: (string_lit) @string.special)
(ephemeral_block type: (string_lit) @type)
(ephemeral_block name: (string_lit) @string.special)
(variable_block name: (string_lit) @variable.parameter)
(output_block name: (string_lit) @variable)
(module_block name: (string_lit) @module)
(provider_block name: (string_lit) @type)
(check_block name: (string_lit) @string.special)
