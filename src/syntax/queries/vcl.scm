;; VCL (Varnish and Fastly dialects). Started from ntsk/tree-sitter-vcl
;; v0.4.1 `queries/highlights.scm` (MIT) and extended for the Fastly
;; constructs added in tico's fork of the grammar; see
;; grammars/tree-sitter-vcl/README.md.
;;
;; Ordering matters: tico applies captures in query order and a later
;; capture of the same node overrides an earlier one, so the generic
;; patterns come first and the specific ones last.

;; Keywords
[
  "vcl"
  "backend"
  "probe"
  "acl"
  "sub"
  "director"
  "table"
  "penaltybox"
  "ratecounter"
  "pragma"
  "set"
  "add"
  "unset"
  "remove"
  "call"
  "new"
  "declare"
  "local"
  "error"
  "synthetic"
  "synthetic.base64"
  "log"
  "restart"
  "esi"
  "goto"
] @keyword

[
  "import"
  "include"
  "from"
] @keyword.control.import

[
  "if"
  "elsif"
  "elseif"
  "else"
] @keyword.control.conditional

"return" @keyword.control.return

;; Operators
[
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "|="
  "&="
  "^="
  "<<="
  ">>="
  "rol="
  "ror="
  "&&="
  "||="
  "=="
  "!="
  "~"
  "!~"
  "<"
  "<="
  ">"
  ">="
  "&&"
  "||"
  "!"
  "+"
  "-"
  "*"
  "/"
  "%"
] @operator

;; Delimiters
[
  ";"
  "."
  ","
  ":"
] @punctuation.delimiter

[
  "("
  ")"
  "{"
  "}"
] @punctuation.bracket

;; Literals
(string) @string
(long_string) @string
(integer) @constant.numeric.integer
(float) @constant.numeric.float
(duration) @constant.numeric
(percentage) @constant.numeric
(version) @constant.numeric
(boolean) @constant.builtin.boolean

;; Varnish inline C. A fallback only: tico re-highlights the body with
;; the C grammar and colors the `C{` / `}C` delimiters itself (see
;; `inject_inline_c` in ../mod.rs).
(inline_c) @string

;; Types: `declare local var.x STRING;`, `table t BACKEND { }`, `sub f BOOL { }`
(type) @type

;; Comments, and Fastly's `#FASTLY recv` boilerplate markers, which are
;; directives to Fastly's VCL compiler rather than commentary.
(comment) @comment
((comment) @keyword.directive
  (#match? @keyword.directive "^#FASTLY"))

;; Identifiers
(identifier) @variable

;; Property names: `.host`, `.probe`, `.quorum`, `.backend`
(property_name) @variable.other.member

;; Member expressions: `req.http.Host`
(member_expression
  (identifier) @variable)

;; The first segment of a member expression is a built-in object or a
;; module namespace.
((identifier) @variable.builtin
  (#match? @variable.builtin "^(req|bereq|beresp|resp|obj|client|server|storage|now|var|fastly|fastly_info|time|geoip|tls|waf)$"))

;; Function calls and subroutine declarations
(subroutine_declaration
  name: (identifier) @function)

(call_statement
  (identifier) @function)

(call_expression
  (identifier) @function)

;; `std.tolower(...)`, `table.lookup(...)`: module namespace, then function.
(call_expression
  (member_expression
    . (identifier) @namespace))

(call_expression
  (member_expression
    (identifier) @function .))

;; Built-in subroutines: `sub vcl_recv`, `call vcl_foo`
((identifier) @function.builtin
  (#match? @function.builtin "^vcl_"))

;; `return (lookup);`, `return (pass);`
(return_statement
  (parenthesized_expression
    (identifier) @constant))

;; Labels: `goto done;` ... `done:`
(goto_statement
  (identifier) @label)

(label_statement
  name: (identifier) @label)

;; Declarations
(acl_declaration
  (identifier) @type)

(director_declaration
  type: (identifier) @type)
