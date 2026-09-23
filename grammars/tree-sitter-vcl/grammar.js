/**
 * @file Varnish Configuration Language grammar for tree-sitter, extended
 *   with the Fastly VCL dialect.
 * @author ntsk <ntsk@ntsk.jp>
 * @license MIT
 *
 * Forked from ntsk/tree-sitter-vcl v0.4.1 for tico. The Fastly additions
 * (table, director, penaltybox, ratecounter, pragma, declare local, error,
 * synthetic, log, goto/labels, add, remove, restart, esi, compound
 * assignment operators, the if() function, string concatenation by
 * juxtaposition, typed subroutines, header subfields, parenthesized
 * expressions, floats and percentages) are marked "Fastly" below.
 */

/// <reference types="tree-sitter-cli/dsl" />

const ASSIGNMENT_OPERATORS = [
  '=', '+=', '-=', '*=', '/=', '%=', '|=', '&=', '^=', '<<=', '>>=',
  'rol=', 'ror=', '&&=', '||=',
];

module.exports = grammar({
  name: "vcl",

  word: $ => $.identifier,

  extras: $ => [
    /\s/,
    $.comment,
  ],

  rules: {
    source_file: $ => repeat($._statement),

    _statement: $ => choice(
      $.vcl_declaration,
      $.import_statement,
      $.include_statement,
      $.pragma_declaration,
      $.backend_declaration,
      $.probe_declaration,
      $.acl_declaration,
      $.director_declaration,
      $.table_declaration,
      $.penaltybox_declaration,
      $.ratecounter_declaration,
      $.subroutine_declaration,
      $.inline_c,
    ),

    comment: $ => token(choice(
      seq('#', /.*/),
      seq('//', /.*/),
      seq('/*', /[^*]*\*+([^/*][^*]*\*+)*/, '/'),
    )),

    vcl_declaration: $ => seq(
      'vcl',
      field('version', $.version),
      ';'
    ),

    version: $ => /\d+\.\d+/,

    import_statement: $ => seq(
      'import',
      $.identifier,
      optional(seq('from', $.string)),
      ';'
    ),

    include_statement: $ => seq(
      'include',
      $.string,
      ';'
    ),

    // Fastly: `pragma optional_param geoip_opt_in true;`
    pragma_declaration: $ => seq(
      'pragma',
      $.identifier,
      repeat(choice(
        $.identifier,
        $.string,
        $.integer,
        $.float,
        $.duration,
        $.boolean,
      )),
      ';'
    ),

    backend_declaration: $ => seq(
      'backend',
      $.identifier,
      '{',
      field('properties', $.backend_properties),
      '}'
    ),

    backend_properties: $ => repeat1($.backend_property),

    backend_property: $ => choice(
      seq(
        field('name', $.property_name),
        '=',
        field('value', $.probe_properties)
      ),
      seq(
        field('name', $.property_name),
        '=',
        field('value', $._expression),
        ';'
      )
    ),

    probe_declaration: $ => seq(
      'probe',
      $.identifier,
      field('properties', $.probe_properties)
    ),

    probe_properties: $ => seq(
      '{',
      repeat($.probe_property),
      '}'
    ),

    probe_property: $ => seq(
      field('name', $.property_name),
      '=',
      field('value', $._expression),
      ';'
    ),

    property_name: $ => /\.[a-zA-Z_][a-zA-Z0-9_]*/,

    acl_declaration: $ => seq(
      'acl',
      $.identifier,
      '{',
      field('entries', $.acl_entries),
      '}'
    ),

    acl_entries: $ => repeat1($.acl_entry),

    acl_entry: $ => seq(
      optional('!'),
      choice(
        $.string,
        seq($.string, '/', $.integer)
      ),
      ';'
    ),

    // Fastly: `director name random { .quorum = 20%; { .backend = b; .weight = 1; } }`
    director_declaration: $ => seq(
      'director',
      field('name', $.identifier),
      field('type', $.identifier),
      '{',
      repeat(choice(
        $.director_property,
        $.director_backend,
      )),
      '}'
    ),

    director_property: $ => seq(
      field('name', $.property_name),
      '=',
      field('value', choice($.percentage, $._expression)),
      ';'
    ),

    director_backend: $ => seq(
      '{',
      repeat($.director_property),
      '}'
    ),

    // Fastly: `table name [TYPE] { "key": value, ... }`
    table_declaration: $ => seq(
      'table',
      field('name', $.identifier),
      optional(field('type', $.type)),
      '{',
      optional(seq(
        $.table_entry,
        repeat(seq(',', $.table_entry)),
        optional(','),
      )),
      '}'
    ),

    table_entry: $ => seq(
      field('key', $.string),
      ':',
      field('value', $._expression),
    ),

    // Fastly
    penaltybox_declaration: $ => seq(
      'penaltybox',
      $.identifier,
      '{',
      '}'
    ),

    // Fastly
    ratecounter_declaration: $ => seq(
      'ratecounter',
      $.identifier,
      '{',
      '}'
    ),

    // Fastly: custom subroutines may declare a return type, `sub f STRING { ... }`.
    subroutine_declaration: $ => seq(
      'sub',
      field('name', $.identifier),
      optional(field('return_type', $.type)),
      $.block
    ),

    // Fastly's variable/table/subroutine types.
    type: $ => choice(
      'ACL',
      'BACKEND',
      'BOOL',
      'FLOAT',
      'INTEGER',
      'IP',
      'REGEX',
      'RTIME',
      'STRING',
      'TIME',
    ),

    block: $ => seq(
      '{',
      repeat($._subroutine_statement),
      '}'
    ),

    _subroutine_statement: $ => choice(
      $.if_statement,
      $.set_statement,
      $.add_statement,
      $.unset_statement,
      $.return_statement,
      $.call_statement,
      $.declaration_statement,
      $.declare_statement,
      $.error_statement,
      $.synthetic_statement,
      $.log_statement,
      $.restart_statement,
      $.esi_statement,
      $.goto_statement,
      $.label_statement,
      $.include_statement,
      $.expression_statement,
      $.inline_c,
    ),

    expression_statement: $ => seq(
      $.call_expression,
      ';'
    ),

    if_statement: $ => prec.right(seq(
      'if',
      field('condition', $.condition),
      field('consequence', $.block),
      repeat(seq(
        choice('elsif', 'elseif', seq('else', 'if')),
        field('condition', $.condition),
        field('alternative', $.block)
      )),
      optional(seq('else', field('alternative', $.block)))
    )),

    condition: $ => seq(
      '(',
      $._expression,
      ')'
    ),

    set_statement: $ => seq(
      'set',
      field('left', $._expression),
      field('operator', choice(...ASSIGNMENT_OPERATORS)),
      field('right', $._expression),
      ';'
    ),

    // Fastly: `add resp.http.Vary = "Accept-Encoding";`
    add_statement: $ => seq(
      'add',
      field('left', $._expression),
      '=',
      field('right', $._expression),
      ';'
    ),

    // Fastly spells `unset` as `remove` too.
    unset_statement: $ => seq(
      choice('unset', 'remove'),
      $._expression,
      ';'
    ),

    // `return (lookup);` / `return (synth(405, "x"));` / Fastly's typed
    // `return "value";`. The parenthesized forms parse as a
    // parenthesized_expression around the action name or call.
    return_statement: $ => seq(
      'return',
      optional($._expression),
      ';'
    ),

    call_statement: $ => seq(
      'call',
      $.identifier,
      ';'
    ),

    declaration_statement: $ => seq(
      'new',
      $.identifier,
      '=',
      $.call_expression,
      ';'
    ),

    // Fastly: `declare local var.name STRING;`
    declare_statement: $ => seq(
      'declare',
      'local',
      field('name', $.member_expression),
      field('type', $.type),
      ';'
    ),

    // Fastly: `error;`, `error 404;`, `error 601 "message";`
    error_statement: $ => seq(
      'error',
      optional($._expression),
      ';'
    ),

    // Fastly: `synthetic {"..."};`, `synthetic.base64 "...";`
    synthetic_statement: $ => seq(
      choice('synthetic', 'synthetic.base64'),
      $._expression,
      ';'
    ),

    // Fastly: `log "syslog " req.service_id " name :: " req.url;`
    log_statement: $ => seq(
      'log',
      $._expression,
      ';'
    ),

    restart_statement: $ => seq('restart', ';'),

    esi_statement: $ => seq('esi', ';'),

    goto_statement: $ => seq(
      'goto',
      $.identifier,
      ';'
    ),

    label_statement: $ => seq(
      field('name', $.identifier),
      ':'
    ),

    call_expression: $ => choice(
      prec(10, seq(
        choice(
          $.member_expression,
          $.identifier
        ),
        $.arguments
      )),
      prec(9, seq(
        $.member_expression,
        '(',
        ')'
      ))
    ),

    arguments: $ => seq(
      '(',
      optional(seq(
        $._expression,
        repeat(seq(',', $._expression))
      )),
      ')'
    ),

    // Fastly: `if(condition, then, else)` as an expression.
    if_expression: $ => seq(
      'if',
      '(',
      field('condition', $._expression),
      ',',
      field('consequence', $._expression),
      ',',
      field('alternative', $._expression),
      ')'
    ),

    _expression: $ => choice(
      $.binary_expression,
      $.unary_expression,
      $.concatenation,
      $.parenthesized_expression,
      $.if_expression,
      $.call_expression,
      $.member_expression,
      $.string,
      $.long_string,
      $.float,
      $.integer,
      $.duration,
      $.boolean,
      $.identifier,
    ),

    parenthesized_expression: $ => seq(
      '(',
      $._expression,
      ')'
    ),

    binary_expression: $ => choice(
      prec.left(1, seq($._expression, '||', $._expression)),
      prec.left(2, seq($._expression, '&&', $._expression)),
      prec.left(3, seq($._expression, '==', $._expression)),
      prec.left(3, seq($._expression, '!=', $._expression)),
      prec.left(3, seq($._expression, '~', $._expression)),
      prec.left(3, seq($._expression, '!~', $._expression)),
      prec.left(4, seq($._expression, '<', $._expression)),
      prec.left(4, seq($._expression, '<=', $._expression)),
      prec.left(4, seq($._expression, '>', $._expression)),
      prec.left(4, seq($._expression, '>=', $._expression)),
      prec.left(5, seq($._expression, '+', $._expression)),
      prec.left(5, seq($._expression, '-', $._expression)),
      prec.left(6, seq($._expression, '*', $._expression)),
      prec.left(6, seq($._expression, '/', $._expression)),
      prec.left(6, seq($._expression, '%', $._expression)),
    ),

    // Fastly: adjacent expressions concatenate as strings,
    // `"prefix " req.url " suffix"`.
    concatenation: $ => prec.left(5, seq($._expression, $._expression)),

    unary_expression: $ => prec(7, seq(
      '!',
      $._expression
    )),

    // `req.http.Host`, plus Fastly's header subfield `req.http.Cookie:name`.
    member_expression: $ => prec.left(8, seq(
      $.identifier,
      repeat1(seq('.', $.identifier)),
      optional(seq(':', field('subfield', $.identifier))),
    )),

    // Varnish inline C, `C{ ... }C`, at top level or inside a subroutine.
    // One token; tico re-highlights the body with its C grammar.
    inline_c: $ => token(seq('C{', /([^}]|\}+[^C}])*\}*/, '}C')),

    string: $ => /"([^"\\]|\\.)*"/,

    long_string: $ => token(choice(
      seq('{"', repeat(choice(/[^"}]/, /"[^}]/)), '"}'),
      seq('"""', /(.|\n|\r)*?/, '"""'),
    )),

    integer: $ => /-?\d+/,

    float: $ => /-?\d+\.\d+/,

    duration: $ => /-?\d+(\.\d+)?(ms|s|m|h|d|w|y)/,

    percentage: $ => /\d+(\.\d+)?%/,

    boolean: $ => choice('true', 'false'),

    identifier: $ => /[a-zA-Z_][a-zA-Z0-9_]*(-[a-zA-Z0-9_]+)*/,
  }
});
