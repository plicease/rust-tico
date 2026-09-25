; From eonpatapon/tree-sitter-cue
; (https://github.com/eonpatapon/tree-sitter-cue, commit dd7b90e),
; queries/highlights.scm. MIT, copyright (c) 2018-2021 Jean-Philippe Braun
; and Amaan Qureshi.
;
; Modified for tico: two patterns are moved earlier. Upstream orders them
; for editors where the first matching pattern wins; tico paints later
; captures over earlier ones, so the generic `(identifier) @variable`
; has to come before the field/property/function/type patterns it would
; otherwise erase, and `[ "<" ">" ] @punctuation.bracket` before the
; operator list so a comparison's `>` stays an operator.

; Includes

[
  "package"
  "import"
] @include

; Namespaces

(package_identifier) @namespace

(import_spec ["." "_"] @punctuation.special)

[
  (attr_path)
  (package_path)
] @text.uri ;; In attributes

; Variables

(identifier) @variable

[ "<" ">" ] @punctuation.bracket

; Attributes

(attribute) @attribute

; Conditionals

[
  "if"
  "else"
] @conditional

; Repeats

[
  "for"
] @repeat

(for_clause "_" @punctuation.special)

; Keywords

[
  "let"
  "try"
] @keyword

[
  "in"
] @keyword.operator

; Operators

[
  "+"
  "-"
  "*"
  "/"
  "|"
  "&"
  "||"
  "&&"
  "=="
  "!="
  "<"
  "<="
  ">"
  ">="
  "=~"
  "!~"
  "!"
  "="
  "~"
] @operator

; Fields & Properties

(field
  (label
  (identifier) @field))

(selector_expression
  (_)
  (identifier) @property)

; Functions

(call_expression
  function: (identifier) @function.call)
(call_expression
  function: (selector_expression
  (_)
  (identifier) @function.call))
(call_expression
  function: (builtin_function) @function.call)

(builtin_function) @function.builtin

; Types

(primitive_type) @type.builtin

((identifier) @type
  (#match? @type "^(#|_#)"))

[
  (slice_type)
  (pointer_type)
] @type ;; In attributes

; Punctuation

[
  ","
  ":"
] @punctuation.delimiter

[ "{" "}" ] @punctuation.bracket

[ "[" "]" ] @punctuation.bracket

[ "(" ")" ] @punctuation.bracket

[
  (ellipsis)
  "?"
  "!"
] @punctuation.special

; Literals

(string) @string

[
  (escape_char)
  (escape_unicode)
] @string.escape

(number) @number

(float) @float

(si_unit
  (float)
  (_) @symbol)

(boolean) @boolean

[
  (null)
  (top)
  (bottom)
] @constant.builtin

; Interpolations

(interpolation "\\(" @punctuation.special (_) ")" @punctuation.special) @none

(interpolation "\\(" (identifier) @variable ")")

; Comments

(comment) @comment @spell

; Errors

(ERROR) @error
