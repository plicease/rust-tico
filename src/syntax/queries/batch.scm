; From the tree-sitter-batch crate (v0.11.1,
; https://github.com/wharflab/tree-sitter-batch), queries/highlights.scm.
; MIT, copyright (c) 2026 WharfLab.
;
; Modified for tico: the generic `(variable_reference) @variable` pattern
; is moved above the `@variable.builtin` one. Upstream orders them for
; Helix, where the first matching pattern wins; tico paints later
; captures over earlier ones, so the specific pattern has to come last
; for `%ERRORLEVEL%` to keep its builtin scope.

; Echo off/on
(echo_off) @keyword

; Comments
(comment) @comment

; Labels
(label) @label

; Variable assignment
(set_keyword) @keyword
(variable_name) @variable
(set_option) @constant
(assignment_literal) @string
(arithmetic_expression) @string

; IF/FOR/GOTO/CALL statements
(if_stmt) @keyword
(for_stmt) @keyword
(goto_stmt) @keyword
(call_stmt) @keyword
(setlocal_stmt) @keyword
(endlocal_stmt) @keyword
(exit_stmt) @keyword

; Operators
(comparison_op) @operator
(redirect_op) @operator
(fd_redirect) @operator

; Commands
(command_name) @function

; Variables
(variable_reference) @variable

; CMD pseudo/dynamic environment variables
((variable_reference) @variable.builtin
  (#match? @variable.builtin "(?i)^[%!](CD|ERRORLEVEL|DATE|TIME|RANDOM|CMDCMDLINE|CMDEXTVERSION|HIGHESTNUMANODENUMBER|__APPDIR__|__CD__)[%!]$"))

; FOR set literal content
(for_set_literal) @string

; FOR loop variables
(for_variable) @variable.parameter

; FOR options
(for_options) @constant

; Strings
(string) @string

; Numbers
(integer) @number

; Command options/flags
(command_option) @constant

; Argument values
(argument_value) @string

; Redirect targets
(redirect_target) @string.special
