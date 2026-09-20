; Hand-written for tico: tree-sitter-perl (crates.io) does not bundle a
; highlights.scm (unlike most other grammars used here, which are vendored
; from their own crates' queries/highlights.scm). Node names taken from
; that crate's src/node-types.json.

(comments) @comment

(string_double_quoted) @string
(string_single_quoted) @string
(string_q_quoted) @string
(string_qq_quoted) @string
(word_list_qw) @string
(command_qx_quoted) @string
(heredoc_body_statement) @string
(heredoc_start_identifier) @string
(heredoc_end_identifier) @string

(regex_pattern_qr) @string.special
(pattern_matcher_m) @string.special
(regex_pattern_content) @string.special
(substitution_pattern_s) @string.special
(transliteration_tr_or_y) @string.special

(scalar_variable) @variable
(array_variable) @variable
(hash_variable) @variable
(package_variable) @variable
(special_scalar_variable) @variable.builtin
(standard_input_to_variable) @variable.builtin

(function_definition name: (identifier) @function)
(function_definition_without_sub name: (identifier) @function)
(call_expression_with_bareword function_name: (identifier) @function)
(call_expression_with_bareword package_name: (package_name) @module)

(package_name) @module

[
  "if" "elsif" "else" "unless"
  "while" "until" "for" "foreach" "continue"
  "sub" "return" "func" "method"
  "my" "our" "local" "state"
  "use" "no" "require" "package" "import" "parent" "feature" "constant" "prototype" "subs" "isa"
  "last" "next" "redo" "goto"
  "and" "or" "not" "xor" "eq" "ne" "lt" "gt" "le" "ge" "cmp"
  "BEGIN" "END" "INIT" "CHECK" "UNITCHECK"
  "bless" "when"
] @keyword
