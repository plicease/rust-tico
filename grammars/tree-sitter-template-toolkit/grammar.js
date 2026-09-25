/**
 * @file Template Toolkit 2 grammar for tree-sitter
 * @author Ruvym Sypa
 * @license MIT
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

/**
 * Chomp flags sit immediately inside either delimiter and control the
 * whitespace around the tag:
 *
 *   +  CHOMP_NONE      keep surrounding whitespace
 *   -  CHOMP_ONE       remove one newline
 *   =  CHOMP_COLLAPSE  collapse surrounding whitespace to a single space
 *   ~  CHOMP_GREEDY    remove all adjacent whitespace
 */
const CHOMP = /[-+=~]/;

// The three delimiters, as functions so that each use site gets its own token
// object. Tree-sitter folds identical tokens into one terminal, so reusing them
// outside a named rule costs nothing and keeps the nested-tag machinery below
// from inventing a second, subtly different notion of what a tag looks like.
const tagOpen = () => token(seq('[%', optional(CHOMP)));
const commentTagOpen = () => token(seq('[%', optional(CHOMP), '#'));
// `prec(2)` puts the closing tag above every token that might otherwise eat its
// first character — in particular the single characters an end-of-line comment
// is built from, which have to outrank the arithmetic operators without
// outranking this.
const tagClose = () => token(prec(2, seq(optional(CHOMP), '%]')));

/**
 * Characters inside a comment. Stops at `[` so that a nested `[%` can be seen
 * as the start of a tag rather than swallowed as text, and treats `%` and the
 * chomp characters one at a time so the closing tag can outbid them.
 *
 * The runs carry `prec(1)` — above the whitespace extra — so that a stretch of
 * text made only of whitespace still belongs to the comment instead of being
 * dissolved into extras.
 */
function commentChars() {
  return [
    token(prec(1, /[^%\-+=~\[]+/)),
    token(prec(-1, CHOMP)),
    token(prec(-1, '%')),
    token(prec(-1, '[')),
  ];
}

/**
 * Operator binding power. Higher binds tighter.
 *
 * `filter` sits at the bottom so that `a + b | html` filters the sum rather
 * than just `b`, which is how the chained-filter examples in the manual read.
 */
const PREC = {
  filter: 1,
  ternary: 2,
  or: 3,
  and: 4,
  equality: 5,
  comparison: 6,
  range: 7,
  concat: 8,
  additive: 9,
  multiplicative: 10,
  unary: 11,
  path: 12,
};

// Directive keywords. Uppercase is the only thing separating them from ordinary
// variable names.
//
// Taken from the ANYCASE list in Template::Manual::Config and @RESERVED in
// Template::Grammar, which between them are the authority. Note the aliases:
// FOR is FOREACH and BREAK is LAST, so both have to be here or the block rules
// in stage 3 will not recognise a loop written the short way.
//
// AND, OR, NOT, MOD and DIV are reserved too, but they are operators here and
// live in the binary/unary rules instead. So is TO, the word form of the range
// `..`, which has to work inside a list literal where a bare keyword would not
// be allowed. STEP is *not* one of them — it is reserved, but the range
// production is `sterm TO sterm` and has no STEP in it, so it stays a keyword.
const KEYWORDS = [
  'GET', 'SET', 'CALL', 'DEFAULT', 'BLOCK', 'IF', 'ELSIF', 'ELSE', 'UNLESS',
  'SWITCH', 'CASE', 'FOR', 'FOREACH', 'WHILE', 'FILTER', 'USE', 'PLUGIN',
  'MACRO', 'PERL', 'RAWPERL', 'TRY', 'THROW', 'CATCH', 'FINAL', 'NEXT',
  'LAST', 'BREAK', 'RETURN', 'STOP', 'CLEAR', 'META', 'TAGS', 'DEBUG',
  'VIEW', 'END', 'IN', 'STEP',
];

/** The four directives whose first argument is a template name rather than an
 *  expression. They are kept out of KEYWORDS because that argument needs its
 *  own lexical rule — see `template_name`. */
const INCLUDE_KEYWORDS = ['INCLUDE', 'PROCESS', 'INSERT', 'WRAPPER'];

/**
 * Directives that open a block closed by `END`, from the `condition`, `loop`,
 * `switch`, `try`, `wrapper`, `filter`, `perl`, `rawperl`, `view` and
 * `defblock` productions of TT2's own yacc grammar (`parser/Parser.yp`).
 *
 * Every one of these also has a *postfix* form that opens nothing —
 * `atomexpr IF expr`, `atomexpr FOR loopvar`, `atomexpr FILTER lnameargs`. The
 * only thing separating the two is position: a block is opened when the keyword
 * *starts* the directive, and `[% "Danger" IF atrisk %]` is an expression with
 * a condition hung off it.
 */
const BLOCK_KEYWORDS = [
  'IF', 'UNLESS', 'FOR', 'FOREACH', 'WHILE', 'SWITCH', 'TRY', 'WRAPPER',
  'FILTER', 'PERL', 'VIEW', 'BLOCK',
];

/**
 * `RAWPERL` is deliberately not in the list above, because its body is not a
 * template. The two productions differ in exactly this:
 *
 *     perl:      PERL ';' block END
 *     rawperl:   RAWPERL ';' TEXT END
 *
 * `block` is parsed template chunks — and the manual says so outright: "The
 * PERL block may contain other template directives. These are processed before
 * the Perl code is evaluated." `TEXT` is a single opaque terminal, and the
 * RAWPERL section makes no such claim. So a `[% … %]` inside RAWPERL is Perl
 * source, not a directive, and treating it as one would highlight a string
 * literal as template code.
 */
const RAW_BLOCK_KEYWORD = 'RAWPERL';

/** Keywords that continue an open block rather than opening or closing one:
 *  the `else`, `case` and `final` chains of the same grammar. */
const BRANCH_KEYWORDS = ['ELSIF', 'ELSE', 'CASE', 'CATCH', 'FINAL'];

const KEYWORDS_EXCEPT_END = KEYWORDS.filter(word => word !== 'END');

const BLOCK_KEYWORDS_EXCEPT_WRAPPER = BLOCK_KEYWORDS.filter(
  word => word !== 'WRAPPER',
);

/**
 * Keywords allowed to *start* an ordinary directive — which is to say, none of
 * the ones that open, continue or close a block.
 *
 * Excluding them is what makes blocks work at all, and the reason is worth
 * spelling out because two gentler attempts failed first. A tag that opens a
 * block also reads as a plain directive, and that reading is complete and
 * error-free, so the parser had no reason to prefer the block; dynamic
 * precedence did not shift it. Worse, `[% ELSE %]` then lexed as a *variable*
 * named ELSE, because lexing is context-aware and the ELSE keyword token is not
 * offered where no branch can start — so even removing keyword extraction
 * changed nothing.
 *
 * The competing reading has to be impossible, not merely unattractive. With
 * these words barred from the front of a directive, `[% END %]` can only be a
 * `block_end`, and the flat reading of `[% IF x %]…[% END %]` dies at the END
 * tag, leaving the block as the only parse that completes.
 */
const KEYWORDS_STARTING_A_DIRECTIVE = KEYWORDS.filter(
  word => word !== 'END'
    && word !== RAW_BLOCK_KEYWORD
    && !BRANCH_KEYWORDS.includes(word)
    && !BLOCK_KEYWORDS.includes(word),
);

function commaSep(rule) {
  return optional(seq(rule, repeat(seq(',', rule))));
}

module.exports = grammar({
  name: 'template_toolkit',

  // Whitespace is skipped everywhere. This is what keeps the expression rules
  // below readable — the alternative is threading an explicit optional-space
  // through every sequence point, which is where such grammars go to die.
  //
  // The risk it introduces is specific and worth naming: an extra that lands
  // *outside* a `content` node would punch a hole in the document handed to the
  // HTML injection, gluing together text that the template kept apart. The
  // defence is that every run of content, comment and string text is matched by
  // a token that itself contains whitespace and carries `prec(1)`, so it always
  // outbids the single-character extra. `test/check-ranges.mjs` verifies the
  // property directly rather than trusting this argument.
  extras: () => [/\s/],

  // The one thing the DFA genuinely cannot do: find where a RAWPERL body ends.
  // It has to run to the exact `[% END %]` and no earlier tag, which needs
  // unbounded lookahead. The hidden external is allowed to return several
  // fragments so it can inspect every ambiguous `[`, while the public
  // `raw_content` rule below keeps the body one node. A separate node type from
  // `content`, too — the editor injects HTML into `content`, and this is Perl.
  // See `src/scanner.c`.
  //
  // `_error_sentinel` appears in no rule, so the only situation in which the
  // parser calls it valid is error recovery, where every external token is
  // offered at once. The scanner uses it as a flag to stand down. Without it,
  // a broken string elsewhere in the file sent the parser into recovery and the
  // scanner swallowed everything after it as Perl.
  externals: $ => [$._raw_content_fragment, $._error_sentinel],

  // No `word` rule, deliberately.
  //
  // Keyword extraction would be the conventional choice, but it gives keywords
  // a silent fallback: where the keyword token is not valid in the current
  // state, the text lexes as an identifier instead. That fallback quietly
  // defeated the block rules — `[% ELSE %]` sitting outside any branch position
  // parsed as a variable named ELSE, which is a perfectly error-free reading, so
  // the branch never formed and no amount of dynamic precedence changed it.
  //
  // Plain longest-match already gives the property `word` is usually wanted
  // for: `INCLUDED` beats `INCLUDE` on length and stays an identifier, while a
  // bare `INCLUDE` ties on length and loses to the string literal only on
  // specificity — which is the right way round. And TT2 genuinely reserves
  // these words, so refusing them as variable names matches the language.

  // Seeing `[% IF`, the parser cannot yet know whether this tag opens a block
  // or is an ordinary directive that merely starts with a keyword — the inline
  // form `[% IF x; y; END %]` is the latter, and nothing distinguishable has
  // been read yet. Declaring the conflict lets tree-sitter carry both readings
  // forward and keep whichever survives to the end of the tag.
  // `WRAPPER` is the awkward one: it both opens a block and takes a template
  // name, so after `[% WRAPPER foo` the parser cannot tell the block-opening
  // reading from the ordinary include-style one either.
  conflicts: $ => [
    // `WRAPPER` is the awkward one: it both opens a block and takes a template
    // name, so after `[% WRAPPER foo` neither reading can be ruled out yet.
    [$._block_item, $.include_statement],
    // A capture prefix against an ordinary assignment: `[% x = ` is both, and
    // which one it is only becomes clear at the keyword that follows.
    [$.capture_prefix, $.assignment],
    // `[% MACRO name …` — the head of a block, or an ordinary MACRO directive.
    // Only the keyword after the name decides, and it has not arrived yet.
    [$.macro_prefix, $._directive_first_item],
    // A keyword inside a block-opening tag may be a plain item or the head of a
    // nested inline block. Whether an `END` arrives before the closing tag is
    // what tells them apart.
    [$._block_item, $.inline_block],
    // `MACRO name` may head a nested inline block or be an ordinary item; the
    // keyword after the name decides.
    [$._block_item, $.macro_prefix],
    // A block keyword in a non-first position: the head of an inline block, or
    // the postfix form in `[% "text" IF cond %]`, which opens nothing. Whether
    // an `END` follows is the difference.
    [$.inline_block, $.keyword],
    [$.macro_prefix, $.keyword],
  ],

  rules: {
    template: $ => repeat($._node),

    _node: $ => choice(
      $.comment_directive,
      $.raw_block,
      $.block,
      $.directive,
      $.content,
    ),

    // `[% RAWPERL %] … [% END %]`. The body is one opaque run of Perl source,
    // not template chunks — see RAW_BLOCK_KEYWORD above. A `[% … %]` appearing
    // inside is therefore left alone rather than parsed as a directive, which
    // is also what TT2 does: its grammar wants a single `TEXT` there, so a
    // directive in that position does not compile.
    // `raw_content` comes from the external scanner, which stops in front of
    // the exact `[% END %]` tag. The closing tag is then the ordinary
    // `block_end`, so it keeps its `tag_open`, `keyword` and `tag_close` — and
    // accepts the `#` comment TT2 allows in any directive.
    raw_block: $ => seq(
      $.raw_block_start,
      optional($.raw_content),
      $.block_end,
    ),

    // Kept as one public node even when the scanner has to return several
    // fragments around ambiguous `[` characters. The fragments are hidden so
    // highlighting and Perl injection see the same stable range as before.
    raw_content: $ => repeat1($._raw_content_fragment),

    raw_block_start: $ => seq(
      $.tag_open,
      optional($.capture_prefix),
      optional($.macro_prefix),
      alias(RAW_BLOCK_KEYWORD, $.keyword),
      repeat($._block_item),
      $.tag_close,
    ),


    // `[% IF x %] … [% ELSE %] … [% END %]` as one nested node. Nesting is what
    // makes indentation, folding, the outline panel and `IF`↔`END` matching
    // possible at all; a flat list of sibling directives can express none of
    // them.
    //
    // Branches are flat markers inside the body, not nodes owning the text that
    // follows them. That is not a simplification for its own sake — it is what
    // makes them parse at all.
    //
    // Held in a separate repetition after the body, `branch_start` was
    // unreachable from within the body, so at `[% ELSE %]` the ELSE token was
    // not among the lexer's candidates and the word fell back to being an
    // *identifier*. That reading is complete and error-free, and no precedence,
    // static or dynamic, dislodged it. Placed here, a branch is reachable at
    // every point in the body: the keyword lexes as a keyword, and since a plain
    // directive may not begin with one, a single reading remains.
    //
    // Nothing the editor needs is lost. The block still spans `IF` to `END` for
    // folding, indentation and match highlighting, and the branch markers sit
    // exactly where a dedent belongs.
    block: $ => seq(
      $.block_start,
      repeat(choice($._node, $.branch_start)),
      $.block_end,
    ),

    // Two things may sit in front of the keyword that opens a block, and TT2's
    // grammar makes both of them wrap a whole `mdir` — any directive, not just
    // `BLOCK`:
    //
    //     capture:  ident ASSIGN mdir
    //     macro:    MACRO IDENT '(' margs ')' mdir  |  MACRO IDENT mdir
    //     mdir:     directive  |  BLOCK ';' block END
    //
    // So `[% result = IF condition %]`, `[% MACRO greet BLOCK %]` and
    // `[% MACRO fmt(n) PERL %]` all open blocks that `[% END %]` closes.
    block_start: $ => seq(
      $.tag_open,
      optional($.capture_prefix),
      optional($.macro_prefix),
      choice(
        // WRAPPER takes a template name like INCLUDE does, and also opens a
        // block, so it needs its own arm to reach `template_name`.
        seq(
          alias('WRAPPER', $.keyword),
          optional($.template_name),
          repeat($._block_item),
        ),
        seq(
          alias(choice(...BLOCK_KEYWORDS_EXCEPT_WRAPPER), $.keyword),
          repeat($._block_item),
        ),
      ),
      $.tag_close,
    ),

    // `[% poem = BLOCK %]`, `[% result = IF cond %]` — the block's output is
    // captured into a variable.
    capture_prefix: $ => seq(
      field('left', $.variable),
      choice('=', '=>'),
    ),

    macro_prefix: $ => seq(
      alias('MACRO', $.keyword),
      field('name', $.identifier),
      optional($.arguments),
    ),

    branch_start: $ => seq(
      $.tag_open,
      alias(choice(...BRANCH_KEYWORDS), $.keyword),
      repeat($._block_item),
      $.tag_close,
    ),

    block_end: $ => seq(
      $.tag_open,
      alias('END', $.keyword),
      repeat($._block_item),
      $.tag_close,
    ),

    // Body items of a tag that opens, continues or closes a block. `END` is
    // excluded, and that exclusion is the whole mechanism for the inline form:
    // TT2 lets a complete block live inside one tag —
    //
    //     [% IF title; INCLUDE header; ELSE; INCLUDE other; END %]
    //
    // — and such a tag must not be read as opening a block that goes hunting
    // for an `END` somewhere later in the file. Barring `END` here means the
    // whole tag cannot match `block_start`, so it falls back to the ordinary
    // directive rule, where `IF` and `END` are simply keywords.
    // `inline_block` is here so that inline blocks nest: in
    // `[% FOREACH a; IF b; END; END %]` the inner `END` has to close the inner
    // `IF`, not the outer loop. Without the recursion the first `END` ended
    // everything and the second was left orphaned.
    _block_item: $ => choice(
      $.inline_block,
      $.include_statement,
      alias(choice(...KEYWORDS_EXCEPT_END), $.keyword),
      $.assignment,
      $._expression,
      $.eol_comment,
      ';',
    ),

    // Everything outside a tag. A bare `[` is ordinary text; only `[%` opens a
    // tag, and `tag_open` takes it because the single-character fallback here
    // is deprioritised.
    //
    // `prec.right` is what makes a run of text collapse into a *single*
    // `content` node. Without it the parser cannot decide whether two adjacent
    // chunks end one node and begin another, since `content` is itself a
    // repetition sitting inside the top-level repetition. One node per run is
    // not cosmetic: the HTML injection keys off these ranges, and a run split
    // into fragments would be injected as several documents.
    content: _ => prec.right(repeat1(choice(
      token(prec(1, /[^\[]+/)),
      token(prec(-1, '[')),
    ))),

    directive: $ => seq(
      $.tag_open,
      optional(seq(
        $._directive_first_item,
        repeat($._directive_item),
      )),
      $.tag_close,
    ),

    // Only the first item is restricted; everything after it may be any
    // keyword, which is what keeps postfix notation — `[% "text" IF cond %]` —
    // working, since there the keyword is never first.
    _directive_first_item: $ => choice(
      $.inline_block,
      $.include_statement,
      alias(choice(...KEYWORDS_STARTING_A_DIRECTIVE), $.keyword),
      $.assignment,
      $._expression,
      $.eol_comment,
      ';',
    ),

    // A whole block living inside one tag: `[% IF x; INCLUDE a; ELSE; b; END %]`.
    // TT2 allows it, and without this rule such a tag would open a block that
    // then goes hunting for an `END` somewhere later in the file.
    //
    // It costs no ambiguity, unlike the earlier attempts: this reading and
    // `block_start` diverge on whether an `END` arrives before the closing tag,
    // so exactly one of them ever completes. The parser just has to carry both
    // until it finds out, which is what GLR is for.
    // Shares `_block_item` with `block_start` on purpose: identical bodies mean
    // the two readings differ by exactly one token — the trailing `END` — which
    // keeps the parser's job small. Giving them separate item rules made every
    // kind of body element ambiguous between the two.
    inline_block: $ => seq(
      optional($.capture_prefix),
      optional($.macro_prefix),
      alias(choice(...BLOCK_KEYWORDS), $.keyword),
      repeat($._block_item),
      alias('END', $.keyword),
    ),

    // Deliberately permissive: a directive body is a sequence of keywords,
    // assignments and expressions in any order, rather than one rule per
    // directive form.
    //
    // That is a choice, not laziness. It covers every shape in the manual with
    // one rule — `FOREACH item IN list`, `INCLUDE header title="x"`,
    // `MACRO number(n) GET n.chunk(-3)`, `THROW food "missing"`, and the
    // side-effect notation `"text" IF condition` — and it cannot be wrong about
    // arities that no reference implementation is available here to check
    // against. The cost is that nonsense like `[% IF END %]` also parses. We
    // highlight; we do not validate.
    // `inline_block` belongs here as well as in the first position: a tag may
    // hold several directives separated by `;`, so a block can begin at any of
    // them — `[% IF a; 'x'; END; IF b; 'y'; END %]` and
    // `[% SET v = 1; IF c; 'x'; END %]` both have one that is not first.
    _directive_item: $ => choice(
      $.inline_block,
      $.include_statement,
      $.keyword,
      $.assignment,
      $._expression,
      $.eol_comment,
      ';',
    ),

    keyword: _ => choice(...KEYWORDS),

    // `INCLUDE might/not/exist` — the argument is a path, not arithmetic.
    // Without this the slashes read as division and `not` as the unary
    // operator, which is exactly how the first draft failed.
    //
    // `template_name` is confined to this rule rather than being one more kind
    // of expression, and that confinement is the whole point: lexing is
    // context-aware, so the token is only offered here. Were it available
    // everywhere, plain `a/b` division would lex as a path.
    include_statement: $ => prec.right(seq(
      alias(choice(...INCLUDE_KEYWORDS), $.keyword),
      optional(choice(
        seq($.template_name, repeat(seq('|', $._expression))),
        $._expression,
      )),
    )),

    // Requires at least one slash, so a plain `INCLUDE header` still goes
    // through the ordinary variable rule and highlights like one.
    template_name: _ => token(
      /[A-Za-z_][A-Za-z0-9_.\-]*(\/[A-Za-z_][A-Za-z0-9_.\-]*)+/,
    ),

    // `#` comments to end of line — but a directive's text has already been cut
    // at `%]` by the time TT2 looks for comments, so this must stop there too.
    // Hence the shape: runs with no `%`, then any `%` that is not followed by
    // `]`. Written as one token so the whitespace extra cannot carry it across
    // the newline that is supposed to end it.
    //
    // The final alternative exists for a comment ending in `%`, as in
    // `# value is 100%` — without it the trailing percent falls outside the
    // comment and reads as a modulo operator. It has to swallow the newline
    // along with the percent, because the only way to tell this case from `%]`
    // without lookahead is to consume what follows.
    // `#` comments to end of line. Two things have to hold at once: the comment
    // must not cross `%]`, because TT2 cuts the tag before it ever looks for
    // comments; and it must not cross the newline that ends it.
    //
    // A single token cannot do this. "Text up to the *first* `%]`" is not what
    // maximal munch computes — on `# abc %]` the longest match of "text with no
    // `%]` inside" is `# abc %`, which swallows the delimiter's percent. There
    // is no lookahead to fix it with, since tokens compile to a DFA.
    //
    // A sequence of `token.immediate` pieces solves both. Immediate tokens
    // refuse to match across skipped whitespace, so once the newline is eaten as
    // an extra the repetition simply cannot continue — which is what ends the
    // comment. And breaking the text into pieces puts `%` back in competition
    // with `%]` one character at a time, where precedence can decide.
    //
    // The precedences are a three-way ordering, not a pair. The single
    // characters must beat the arithmetic operators — `-`, `+`, `=` and `~` are
    // all tokens in their own right, and at `prec(-1)` a comment containing
    // `a - b` ended at the minus — while still losing to `tag_close`, which sits
    // at `prec(2)` for exactly this reason.
    eol_comment: _ => seq(
      '#',
      repeat(choice(
        token.immediate(prec(1, /[^%\r\n\-+=~]+/)),
        token.immediate(prec(1, /[%\-+=~]/)),
      )),
    ),

    // ---------------------------------------------------------------- values

    _expression: $ => choice(
      $.variable,
      $.string,
      $.number,
      $.list,
      $.hash,
      $.parenthesized_expression,
      $.unary_expression,
      $.binary_expression,
      $.ternary_expression,
    ),

    // A dotted path: `user.name`, `list.0.key`, `people.sort('surname').first`.
    // Only later segments may be numeric — a leading number is a literal, not
    // the head of a path.
    variable: $ => prec.left(PREC.path, seq(
      $._path_root,
      repeat(seq('.', $._path_segment)),
    )),

    // `prec.right` settles a genuine ambiguity created by the permissive body
    // rule above: in `[% foo (a) %]`, the `(` could open this name's argument
    // list or start a separate parenthesised expression sitting next to it.
    // Attaching it as a call is what every real template means.
    _path_root: $ => prec.right(seq(
      choice($.identifier, $.interpolation),
      optional($.arguments),
    )),

    _path_segment: $ => prec.right(seq(
      choice($.identifier, $.number, $.interpolation),
      optional($.arguments),
    )),

    // `page.$pagename` and `users.${me.id}.name` — a path segment named by
    // another variable.
    interpolation: $ => choice(
      seq('$', $.identifier),
      seq('${', $._expression, '}'),
    ),

    arguments: $ => seq(
      '(',
      commaSep(choice($.assignment, $._expression)),
      ')',
    ),

    list: $ => seq('[', commaSep($._expression), ']'),

    // Commas between pairs are optional, and `=` stands in for `=>`.
    hash: $ => seq('{', repeat(choice($.pair, ',')), '}'),

    pair: $ => seq(
      field('key', choice($.identifier, $.number, $.string, $.interpolation)),
      choice('=', '=>'),
      field('value', $._expression),
    ),

    assignment: $ => prec.right(seq(
      field('left', $.variable),
      choice('=', '=>'),
      // The right-hand side may be a directive rather than a value —
      // `[% headtext = PROCESS header %]` captures a directive's output. The
      // keyword is taken here and its arguments fall to the surrounding
      // repetition, which is enough for highlighting.
      field('right', choice($._expression, $.keyword, $.include_statement)),
    )),

    // ------------------------------------------------------------- operators

    // An assignment is allowed here, not just a value: TT2 documents
    // `[% WHILE (user = get_next_user_record) %]`, where the parentheses exist
    // precisely to make the assignment an expression.
    parenthesized_expression: $ => seq(
      '(',
      choice($._expression, $.assignment),
      ')',
    ),

    unary_expression: $ => prec(PREC.unary, seq(
      field('operator', choice('!', 'not', 'NOT', '-')),
      field('operand', $._expression),
    )),

    binary_expression: $ => choice(
      ...[
        ['|', PREC.filter],
        ['||', PREC.or], ['or', PREC.or], ['OR', PREC.or],
        ['&&', PREC.and], ['and', PREC.and], ['AND', PREC.and],
        ['==', PREC.equality], ['!=', PREC.equality],
        ['<', PREC.comparison], ['>', PREC.comparison],
        ['<=', PREC.comparison], ['>=', PREC.comparison],
        ['<=>', PREC.comparison],
        // `..` and its word form `TO`. `STEP` is *not* here: the range
        // production in TT2's grammar is `sterm TO sterm` and nothing more, so
        // `[ 1 TO 10 STEP 2 ]` is an extension this grammar invented. It is a
        // reserved word all the same, and lives in KEYWORDS.
        ['..', PREC.range], ['TO', PREC.range],
        ['_', PREC.concat],
        ['+', PREC.additive], ['-', PREC.additive],
        ['*', PREC.multiplicative], ['/', PREC.multiplicative],
        ['%', PREC.multiplicative],
        ['div', PREC.multiplicative], ['DIV', PREC.multiplicative],
        ['mod', PREC.multiplicative], ['MOD', PREC.multiplicative],
      ].map(([operator, precedence]) => prec.left(precedence, seq(
        field('left', $._expression),
        field('operator', operator),
        field('right', $._expression),
      ))),
    ),

    ternary_expression: $ => prec.right(PREC.ternary, seq(
      field('condition', $._expression),
      '?',
      field('consequence', $._expression),
      ':',
      field('alternative', $._expression),
    )),

    // ---------------------------------------------------------------- atoms

    string: $ => choice($.single_quoted_string, $.double_quoted_string),

    // Both string forms refuse to cross `%]`, on purpose. TT2 cuts tags with a
    // non-greedy match before it knows anything about quotes, so
    // `[% x = "a %] b" %]` is an unterminated string to the engine, not a
    // string containing the delimiter. Letting the string win would paint it as
    // valid and hide a template that dies at render time — and, worse, an
    // unterminated quote would run to the next quote anywhere in the file,
    // desyncing everything after it. That is not a rare state: it is what every
    // string looks like halfway through being typed. Refusing to cross the
    // delimiter contains the damage to one directive.
    //
    // Precedence cannot express this. Lexing is context-aware *before* it is
    // precedence-aware, so inside a string `tag_close` is not even a candidate
    // token and cannot outbid anything. The stop has to be built into the
    // string's own tokens, and with no lookahead available the shape is:
    //
    //   - runs of ordinary characters, which never contain `%`
    //   - one or more `%` *followed by an ordinary character* — so `%]` is not
    //     one of them
    //   - a terminator that folds any trailing `%` into the closing quote,
    //     which is what keeps `"100%"` working
    single_quoted_string: $ => seq(
      "'",
      repeat(choice(
        token(prec(1, /[^'\\%]+/)),
        token(prec(-1, /%+[^'\\%\]]/)),
        $.escape_sequence,
      )),
      token(/%*'/),
    ),

    double_quoted_string: $ => seq(
      '"',
      repeat(choice(
        token(prec(1, /[^"\\%$]+/)),
        token(prec(-1, /%+[^"\\%$\]]/)),
        token(prec(-1, '$')),
        $.escape_sequence,
        $.interpolation,
      )),
      token(/%*"/),
    ),

    escape_sequence: _ => token(seq('\\', /./)),

    number: _ => token(/\d+(\.\d+)?/),

    identifier: _ => token(/[A-Za-z_][A-Za-z0-9_]*/),

    // ------------------------------------------------------------- comments

    tag_open: _ => tagOpen(),
    comment_tag_open: _ => commentTagOpen(),
    tag_close: _ => tagClose(),

    // A comment does not simply end at the next `%]`: TT2 counts the unbalanced
    // `[%` inside it and swallows one further `%]` for each. That is how a block
    // of template code gets commented out —
    //
    //     [%#
    //       [% INCLUDE header %]
    //     %]
    //
    // — and it is the reason the whole block above is one comment rather than a
    // comment followed by a live INCLUDE.
    //
    // Counting sounds like a job for an external scanner, but it is not: "ends
    // at the first unbalanced `%]`" is exactly balanced nesting, which is
    // context-free and expressible directly as a recursive rule.
    comment_directive: $ => seq(
      $.comment_tag_open,
      optional($.comment_body),
      $.tag_close,
    ),

    comment_body: $ => repeat1(choice(
      ...commentChars(),
      $._comment_nested_tag,
    )),

    // A whole `[% … %]` pair living inside a comment. Hidden, and built from
    // bare tokens rather than the named `tag_open` / `tag_close` rules, so that
    // the comment highlights as one flat span instead of sprouting delimiters
    // that look like live directives.
    _comment_nested_tag: $ => seq(
      choice(commentTagOpen(), tagOpen()),
      repeat(choice(
        ...commentChars(),
        $._comment_nested_tag,
      )),
      tagClose(),
    ),
  },
});
