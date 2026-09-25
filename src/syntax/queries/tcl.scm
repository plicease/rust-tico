; From tree-sitter-grammars/tree-sitter-tcl
; (https://github.com/tree-sitter-grammars/tree-sitter-tcl, commit 850a72a),
; queries/tcl/highlights.scm. MIT, copyright (c) 2022 Lewis Russell.
;
; Modified for tico: upstream stacks a generic fallback capture after each
; specific one (`@keyword.function @keyword`, `@function.builtin
; @function`, ...) for editors that don't resolve scopes by prefix. tico
; paints later captures over earlier ones, so the fallback would win and
; the specific scope would be lost; tico's themes resolve by longest
; prefix anyway, so the fallbacks are dropped here.

(comment) @spell @comment

(command name: (simple_word) @function)

"proc" @keyword.function

(procedure
  name: (_) @variable
)

(set (id) @variable)

(argument
  name: (_) @variable.parameter
)

((simple_word) @variable.builtin
               (#any-of? @variable.builtin
                "argc"
                "argv"
                "argv0"
                "auto_path"
                "env"
                "errorCode"
                "errorInfo"
                "tcl_interactive"
                "tcl_library"
                "tcl_nonwordchars"
                "tcl_patchLevel"
                "tcl_pkgPath"
                "tcl_platform"
                "tcl_precision"
                "tcl_rcFileName"
                "tcl_traceCompile"
                "tcl_traceExec"
                "tcl_wordchars"
                "tcl_version"))


"expr" @function.builtin

; Highlight switch arguments as string
(command
    name: (simple_word) @keyword
    arguments:
        (word_list
            (braced_word
                (command
                    name: (simple_word) @string)))
    (#eq? @keyword "switch"))

(command
  name: (simple_word) @function.builtin
  (#any-of? @function.builtin
   "cd"
   "exec"
   "exit"
   "incr"
   "info"
   "join"
   "puts"
   "regexp"
   "regsub"
   "split"
   "subst"
   "trace"
   "source"))

; Highlight unset and variable arguments as variables
(command
    name: (simple_word) @keyword
    arguments: (word_list) @variable
    (#any-of? @keyword
        "unset"
        "variable"))

(command name: (simple_word) @keyword
         (#any-of? @keyword
          "append"
          "break"
          "catch"
          "continue"
          "default"
          "dict"
          "error"
          "eval"
          "global"
          "lappend"
          "lassign"
          "lindex"
          "linsert"
          "list"
          "llength"
          "lmap"
          "lrange"
          "lrepeat"
          "lreplace"
          "lreverse"
          "lsearch"
          "lset"
          "lsort"
          "package"
          "return"
          "trap"
          "throw"))

[
 "catch"
 "error"
 "global"
 "namespace"
 "on"
 "set"
 "try"
 "finally"
 ] @keyword

(unpack) @operator

[
 "while"
 "foreach"
 ; "for"
 ] @repeat

[
 "if"
 "else"
 "elseif"
 ] @conditional

[
 "**"
 "/" "*" "%" "+" "-"
 "<<" ">>"
 ">" "<" ">=" "<="
 "==" "!="
 "eq" "ne"
 "in" "ni"
 "&"
 "^"
 "|"
 "&&"
 "||"
 ] @operator

(variable_substitution) @variable
(quoted_word) @string
(escaped_character) @string.escape

[
 "{" "}"
 "[" "]"
 ";"
 ] @punctuation.bracket

(number) @number

((simple_word) @number
               (#match? @number
                   "^[0-9]+$|^[+-]?[0-9]+$"))


((simple_word) @boolean
               (#any-of? @boolean "true" "false"))

; after apply array auto_execok auto_import auto_load auto_mkindex auto_qualify
; auto_reset bgerror binary chan clock close coroutine dde encoding eof fblocked
; fconfigure fcopy file fileevent filename flush format gets glob history http
; interp load mathfunc mathop memory msgcat my next nextto open parray pid
; pkg::create pkg_mkIndex platform platform::shell pwd re_syntax read refchan
; registry rename safe scan seek self socket source string tailcall tcl::prefix
; tcl_endOfWord tcl_findLibrary tcl_startOfNextWord tcl_startOfPreviousWord
; tcl_wordBreakAfter tcl_wordBreakBefore tcltest tell time timerate tm
; transchan unknown unload update uplevel upvar vwait yield yieldto zlib
