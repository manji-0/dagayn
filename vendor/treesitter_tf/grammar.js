/**
 * @file Terraform grammar for tree-sitter
 * @license MIT
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const PREC = {
  or: 1,
  and: 2,
  comparative: 3,
  relational: 4,
  additive: 5,
  multiplicative: 6,
  unary: 7,
  member: 8,
  // string/template disambiguation
  string_lit: 2,
  quoted_template: 1,
};

module.exports = grammar({
  name: "terraform",

  externals: ($) => [
    $._quoted_template_start,
    $._quoted_template_end,
    $._template_literal_chunk,
    $._template_interpolation_start,
    $._template_interpolation_end,
    $._template_directive_start,
    $._template_directive_end,
    $.heredoc_identifier,
  ],

  extras: ($) => [$.comment, /\s/],

  word: ($) => $.identifier,

  conflicts: ($) => [
    // String literal vs quoted template (resolved by precedence)
    [$.string_lit, $.quoted_template],
    // identifier followed by '(' is always a function call, not variable_expr + paren_expr
    [$.variable_expr, $.function_name],
  ],

  rules: {
    // =========================================================================
    // Root
    // =========================================================================

    config_file: ($) =>
      repeat(
        choice(
          $.resource_block,
          $.data_block,
          $.variable_block,
          $.output_block,
          $.module_block,
          $.provider_block,
          $.locals_block,
          $.terraform_block,
          $.moved_block,
          $.import_block,
          $.check_block,
          $.removed_block,
          $.ephemeral_block,
        ),
      ),

    // =========================================================================
    // Top-level blocks — each gets a distinct node type
    // =========================================================================

    resource_block: ($) =>
      seq(
        "resource",
        field("type", $.string_lit),
        field("name", $.string_lit),
        field("body", $.block_body),
      ),

    data_block: ($) =>
      seq(
        "data",
        field("type", $.string_lit),
        field("name", $.string_lit),
        field("body", $.block_body),
      ),

    variable_block: ($) =>
      seq(
        "variable",
        field("name", $.string_lit),
        field("body", $.block_body),
      ),

    output_block: ($) =>
      seq(
        "output",
        field("name", $.string_lit),
        field("body", $.block_body),
      ),

    module_block: ($) =>
      seq(
        "module",
        field("name", $.string_lit),
        field("body", $.block_body),
      ),

    provider_block: ($) =>
      seq(
        "provider",
        field("name", $.string_lit),
        field("body", $.block_body),
      ),

    locals_block: ($) => seq("locals", field("body", $.block_body)),

    terraform_block: ($) => seq("terraform", field("body", $.block_body)),

    moved_block: ($) => seq("moved", field("body", $.block_body)),

    import_block: ($) => seq("import", field("body", $.block_body)),

    check_block: ($) =>
      seq("check", field("name", $.string_lit), field("body", $.block_body)),

    removed_block: ($) => seq("removed", field("body", $.block_body)),

    ephemeral_block: ($) =>
      seq(
        "ephemeral",
        field("type", $.string_lit),
        field("name", $.string_lit),
        field("body", $.block_body),
      ),

    // =========================================================================
    // Block body and body items
    // =========================================================================

    block_body: ($) => seq("{", optional($._block_body_inner), "}"),

    _block_body_inner: ($) => repeat1($._body_item),

    _body_item: ($) => choice($.attribute, $.block),

    attribute: ($) =>
      seq(
        field("name", $.identifier),
        "=",
        field("value", $.expression),
        optional($._newline_or_end),
      ),

    // Generic nested block (lifecycle, provisioner, dynamic, etc.)
    block: ($) =>
      seq(
        field("type", $.identifier),
        repeat(field("label", choice($.string_lit, $.identifier))),
        field("body", $.block_body),
      ),

    _newline_or_end: ($) => /\n/,

    // =========================================================================
    // Expressions
    // =========================================================================

    expression: ($) =>
      choice(
        $.literal_value,
        $.template_expr,
        $.collection_value,
        $.variable_expr,
        $.function_call,
        $.for_expr,
        $.operation,
        $.conditional,
        $._expr_with_traversal,
        $.paren_expr,
      ),

    paren_expr: ($) => seq("(", $.expression, ")"),

    _expr_with_traversal: ($) =>
      prec.right(
        PREC.member,
        choice(
          seq($.expression, $.index),
          seq($.expression, $.get_attr),
          seq($.expression, $.splat),
        ),
      ),

    // =========================================================================
    // Literals
    // =========================================================================

    literal_value: ($) =>
      choice($.numeric_lit, $.bool_lit, $.null_lit, $.string_lit),

    numeric_lit: (_) =>
      token(
        choice(
          /[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?/,
          /0x[0-9a-fA-F]+/,
        ),
      ),

    bool_lit: (_) => token(choice("true", "false")),

    null_lit: (_) => "null",

    // string_lit: plain quoted string (no interpolation, higher precedence)
    string_lit: ($) =>
      prec(
        PREC.string_lit,
        seq($._quoted_template_start, optional($._string_lit_content), $._quoted_template_end),
      ),

    _string_lit_content: ($) => repeat1($._template_literal_chunk),

    // =========================================================================
    // Template expressions (strings with interpolation)
    // =========================================================================

    template_expr: ($) => choice($.quoted_template, $.heredoc_template),

    // quoted_template: string with possible interpolation/directives
    quoted_template: ($) =>
      prec(
        PREC.quoted_template,
        seq(
          $._quoted_template_start,
          optional($._quoted_template_body),
          $._quoted_template_end,
        ),
      ),

    _quoted_template_body: ($) =>
      repeat1(
        choice(
          $._template_literal_chunk,
          $.template_interpolation,
          $.template_directive,
        ),
      ),

    // heredoc_template: the scanner handles the entire <<[-]IDENTIFIER opening
    // and the closing IDENTIFIER, so we only need two heredoc_identifier tokens.
    heredoc_template: ($) =>
      seq(
        $.heredoc_identifier,
        optional($._heredoc_body),
        $.heredoc_identifier,
      ),

    _heredoc_body: ($) =>
      repeat1(
        choice(
          $._template_literal_chunk,
          $.template_interpolation,
          $.template_directive,
        ),
      ),

    template_interpolation: ($) =>
      seq(
        $._template_interpolation_start,
        optional($.strip_marker),
        optional($.expression),
        optional($.strip_marker),
        $._template_interpolation_end,
      ),

    template_directive: ($) =>
      choice($.template_for, $.template_if),

    template_for: ($) =>
      seq(
        $._template_directive_start,
        optional($.strip_marker),
        "for",
        field("key", optional(seq($.identifier, ","))),
        field("value", $.identifier),
        "in",
        field("collection", $.expression),
        optional($.strip_marker),
        $._template_directive_end,
        optional($._quoted_template_body),
        $._template_directive_start,
        optional($.strip_marker),
        "endfor",
        optional($.strip_marker),
        $._template_directive_end,
      ),

    template_if: ($) =>
      seq(
        $._template_directive_start,
        optional($.strip_marker),
        "if",
        field("condition", $.expression),
        optional($.strip_marker),
        $._template_directive_end,
        optional($._quoted_template_body),
        optional(
          seq(
            $._template_directive_start,
            optional($.strip_marker),
            "else",
            optional($.strip_marker),
            $._template_directive_end,
            optional($._quoted_template_body),
          ),
        ),
        $._template_directive_start,
        optional($.strip_marker),
        "endif",
        optional($.strip_marker),
        $._template_directive_end,
      ),

    strip_marker: (_) => "~",

    // =========================================================================
    // Variable reference
    // =========================================================================

    variable_expr: ($) => $.identifier,

    // =========================================================================
    // Function call
    // =========================================================================

    function_call: ($) =>
      seq(
        field("name", $.function_name),
        "(",
        optional($.function_arguments),
        ")",
      ),

    function_name: ($) =>
      seq(
        $.identifier,
        optional(seq("::", $.identifier, optional(seq("::", $.identifier)))),
      ),

    function_arguments: ($) =>
      seq(
        $.expression,
        repeat(seq(",", $.expression)),
        optional(choice(",", "...")),
      ),

    // =========================================================================
    // For expressions
    // =========================================================================

    for_expr: ($) => choice($.for_tuple_expr, $.for_object_expr),

    for_tuple_expr: ($) =>
      seq("[", $._for_intro, $.expression, optional($._for_cond), "]"),

    for_object_expr: ($) =>
      seq(
        "{",
        $._for_intro,
        field("key", $.expression),
        "=>",
        field("value", $.expression),
        optional("..."),
        optional($._for_cond),
        "}",
      ),

    _for_intro: ($) =>
      seq(
        "for",
        field("key", optional(seq($.identifier, ","))),
        field("value", $.identifier),
        "in",
        field("collection", $.expression),
        ":",
      ),

    _for_cond: ($) => seq("if", $.expression),

    // =========================================================================
    // Operations
    // =========================================================================

    operation: ($) => choice($.unary_operation, $.binary_operation),

    unary_operation: ($) =>
      prec(
        PREC.unary,
        seq(field("operator", choice("-", "!")), field("operand", $.expression)),
      ),

    binary_operation: ($) => {
      const ops = [
        [PREC.or, "||"],
        [PREC.and, "&&"],
        [PREC.comparative, choice("==", "!=")],
        [PREC.relational, choice(">", ">=", "<", "<=")],
        [PREC.additive, choice("+", "-")],
        [PREC.multiplicative, choice("*", "/", "%")],
      ];
      return choice(
        ...ops.map(([prec_level, op]) =>
          prec.left(
            prec_level,
            seq(
              field("left", $.expression),
              field("operator", op),
              field("right", $.expression),
            ),
          ),
        ),
      );
    },

    // =========================================================================
    // Conditional (ternary)
    // =========================================================================

    conditional: ($) =>
      prec.right(
        seq(
          field("condition", $.expression),
          "?",
          field("true_val", $.expression),
          ":",
          field("false_val", $.expression),
        ),
      ),

    // =========================================================================
    // Collection values
    // =========================================================================

    collection_value: ($) => choice($.tuple, $.object),

    tuple: ($) =>
      seq(
        "[",
        optional(
          seq($.expression, repeat(seq(",", $.expression)), optional(",")),
        ),
        "]",
      ),

    object: ($) =>
      seq(
        "{",
        optional(seq($.object_elem, repeat(seq(optional(","), $.object_elem)), optional(","))),
        "}",
      ),

    object_elem: ($) =>
      seq(
        field("key", $.expression),
        choice("=", ":"),
        field("value", $.expression),
      ),

    // =========================================================================
    // Traversals: index, attribute access, splat
    // =========================================================================

    index: ($) =>
      choice(
        seq("[", field("key", $.expression), "]"),
        seq(".", field("key", $.numeric_lit)),
      ),

    get_attr: ($) => seq(".", field("name", $.identifier)),

    splat: ($) => choice($.attr_splat, $.full_splat),

    attr_splat: ($) =>
      prec.right(seq(".", "*", repeat(choice($.get_attr, $.index)))),

    full_splat: ($) =>
      prec.right(seq("[", "*", "]", repeat(choice($.get_attr, $.index)))),

    // =========================================================================
    // Identifier
    // =========================================================================

    identifier: (_) => /[a-zA-Z_\u00C0-\u00D6\u00D8-\u00F6\u00F8-\u02FF\u0370-\u037D\u037F-\u1FFF\u200C-\u200D\u2070-\u218F\u2C00-\u2FEF\u3001-\uD7FF\uF900-\uFDCF\uFDF0-\uFFFD][a-zA-Z0-9_\-\u00B7\u0300-\u036F\u203F-\u2040\u00C0-\u00D6\u00D8-\u00F6\u00F8-\u02FF\u0370-\u037D\u037F-\u1FFF\u200C-\u200D\u2070-\u218F\u2C00-\u2FEF\u3001-\uD7FF\uF900-\uFDCF\uFDF0-\uFFFD]*/,

    // =========================================================================
    // Comments
    // =========================================================================

    comment: (_) =>
      token(
        choice(
          seq("#", /.*/),
          seq("//", /.*/),
          seq("/*", /[^*]*\*+([^/*][^*]*\*+)*/, "/"),
        ),
      ),
  },
});
