; Copied unmodified from the tree-sitter-containerfile crate (v0.9.2,
; https://github.com/wharflab/tree-sitter-containerfile), queries/highlights.scm.
; MIT, copyright (c) 2026 WharfLab; derived from Camden Cheek's
; tree-sitter-dockerfile (MIT, copyright (c) 2021 Camden Cheek).

[
  "FROM"
  "AS"
  "RUN"
  "CMD"
  "LABEL"
  "EXPOSE"
  "ENV"
  "ADD"
  "COPY"
  "ENTRYPOINT"
  "VOLUME"
  "USER"
  "WORKDIR"
  "ARG"
  "ONBUILD"
  "STOPSIGNAL"
  "HEALTHCHECK"
  "SHELL"
  "MAINTAINER"
  "CROSS_BUILD"
] @keyword

[
  ":"
  "@"
] @operator

(comment) @comment @spell

(image_spec
  (image_tag
    ":" @punctuation.special)
  (image_digest
    "@" @punctuation.special))

[
  (double_quoted_string)
  (single_quoted_string)
  (json_string)
] @string

(heredoc_block) @string

[
  (heredoc_marker)
  (heredoc_end)
] @label

(escape_sequence) @string.escape

(expansion
  [
    "$"
    "{"
    "}"
  ] @punctuation.special
)

(expansion_operator) @operator

((variable) @constant
  (#match? @constant "^[A-Z][A-Z_0-9]*$"))

(arg_pair
  name: (unquoted_string) @property)

(env_pair
  name: (unquoted_string) @property)

(label_pair
  key: (_) @property)

(param
  name: (_) @property)

(mount_param
  name: (_) @property)

(mount_param_param) @property

(expose_port) @number
