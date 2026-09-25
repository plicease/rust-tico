; From the Template Toolkit Zed extension
; (https://github.com/RuvimSypa/template-toolkit-zed, commit 93854ef),
; languages/template-toolkit/highlights.scm, unmodified. MIT, copyright
; (c) 2026 Ruvym Sypa. The stacked pairs (`@string @string.special.path`,
; `@function @function.method`) list the specific capture last, which is
; the one tico keeps.

; Order matters: later patterns win, so the comment rules at the end paint over
; anything the expression rules matched inside a commented-out directive.

; ---------------------------------------------------------------- delimiters

(tag_open) @punctuation.bracket
(tag_close) @punctuation.bracket

; --------------------------------------------------------------- directives

(keyword) @keyword

(macro_prefix
  name: (identifier) @function)

; Two captures, general then specific. Zed resolves them right to left: it tries
; `string.special.path` first and falls back to `string` when the theme has no
; style for it — which the built-in One themes do not.
(template_name) @string @string.special.path

; ---------------------------------------------------------------- variables

(variable (identifier) @variable)

; A path segment carrying an argument list is a call: `people.sort('surname')`.
; Same fallback pairing — `function.method` is not in the One themes either.
(variable
  (identifier) @function @function.method
  . (arguments))

; `page.$pagename` and `users.${me.id}.name`
(interpolation) @punctuation.special
(interpolation (identifier) @variable)

(pair key: (identifier) @property)

; ------------------------------------------------------------------- values

(string) @string
(escape_sequence) @string.escape
(number) @number

; An interpolation inside a string is code again, not string.
(double_quoted_string (interpolation) @punctuation.special)
(double_quoted_string (interpolation (identifier) @variable))

; ---------------------------------------------------------------- operators

(binary_expression operator: _ @operator)
(unary_expression operator: _ @operator)
(ternary_expression ["?" ":"] @operator)
(assignment ["=" "=>"] @operator)
(capture_prefix ["=" "=>"] @operator)
(pair ["=" "=>"] @operator)

["." ","] @punctuation.delimiter
";" @punctuation.delimiter
["(" ")" "[" "]" "{" "}"] @punctuation.bracket

; ----------------------------------------------------------------- comments

; Last, so that a commented-out block stays one flat comment even though the
; grammar keeps the tags it swallowed as ordinary text inside it.
(eol_comment) @comment
(comment_directive) @comment
(comment_tag_open) @comment
(comment_body) @comment
