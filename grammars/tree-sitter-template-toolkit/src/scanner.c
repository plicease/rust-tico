#include "tree_sitter/parser.h"

// The body of a RAWPERL block is opaque Perl source that runs up to the exact
// `[% END %]` tag. Nothing weaker expresses that:
//
//   - stopping the raw text at every `[%` breaks on a literal tag inside Perl,
//     which is the whole point of RAWPERL being raw;
//   - matching the closing tag as one big token fixes that but flattens it,
//     leaving no `tag_open`, `keyword` or `tag_close` for the editor to
//     highlight, and no room for the `#` comment TT2 allows in any directive —
//     `[% END # close RAWPERL %]`.
//
// So the scanner's only job is to find where the raw text stops. It consumes
// characters until it is standing in front of a tag whose first word is `END`,
// and leaves that tag alone; the ordinary `block_end` rule then parses it with
// all its parts intact.

enum TokenType {
  RAW_CONTENT_FRAGMENT,
  // Named in `externals` but used by no rule, so the parser only ever calls it
  // valid when it is offering every external token at once — that is, during
  // error recovery. See the guard at the top of `scan`.
  ERROR_SENTINEL,
};

enum TagEnding {
  NOT_TAG_END,
  TAG_END,
  NESTED_TAG_OPEN,
};

static bool is_chomp_flag(int32_t c) {
  return c == '-' || c == '+' || c == '=' || c == '~';
}

static bool is_space(int32_t c) {
  return c == ' ' || c == '\t' || c == '\r' || c == '\n';
}

static bool is_word_char(int32_t c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
         (c >= '0' && c <= '9') || c == '_';
}

// Consume one character as part of a raw fragment. Keeping the mark current is
// what lets a failed END candidate become Perl text without rescanning it.
static void advance_content(TSLexer *lexer) {
  lexer->advance(lexer, false);
  lexer->mark_end(lexer);
}

// Reads to the tag's closing `%]`, having given up on inspecting what lies
// between. Fails on end of input, and on meeting another `[%` first — that
// second guard is what keeps a Perl string honest: in
//
//     $x = "[% END; is text";
//     [% END %]
//
// the `%]` that would otherwise be found belongs to the *real* closing tag two
// lines down, and stopping at the fake one would cut the block short.
static enum TagEnding scan_to_tag_close(TSLexer *lexer) {
  for (;;) {
    if (lexer->eof(lexer)) return NOT_TAG_END;

    if (lexer->lookahead == '[') {
      // It is safe to look one character ahead because this scan starts at the
      // candidate's `[`. If this is another `[%`, the current fragment ends at
      // the still-marked position before it and the next fragment gets to judge
      // the new candidate. An ordinary `[` belongs to the directive suffix —
      // `[% END; SET values = [1, 2] %]` — and scanning continues.
      lexer->advance(lexer, false);
      if (lexer->lookahead == '%') return NESTED_TAG_OPEN;
      lexer->mark_end(lexer);
      continue;
    }

    if (lexer->lookahead == '%') {
      advance_content(lexer);
      if (lexer->lookahead == ']') return TAG_END;
      continue;
    }

    advance_content(lexer);
  }
}

// Called with the word `END` just consumed. Decides whether the tag really
// closes here by reading on to an actual `%]`, allowing only what TT2 permits
// between the two: whitespace, an optional `#` comment, and an optional chomp
// flag.
//
// Checking merely that `END` is followed by a non-word character is not enough,
// and the gap is easy to fall into: a Perl string reading
// `"[% END is only text"` ended the block on the strength of the space after
// END. Anything that is not a well-formed tag ending has to fall back to being
// Perl source.
//
// The strictness is asymmetric, and deliberately so. With no separator only
// whitespace, a comment and a chomp flag may precede the `%]`: TT2 wants a `;`
// between directives, so loose words after `END` are a Perl string rather than
// a directive, and admitting them would end blocks early. Once a `;` does
// arrive the judgement passes to `block_end`, which is the rule that knows how
// to read what follows.
static enum TagEnding at_tag_ending(TSLexer *lexer) {
  for (;;) {
    if (lexer->eof(lexer)) return NOT_TAG_END;

    while (is_space(lexer->lookahead)) {
      advance_content(lexer);
      if (lexer->eof(lexer)) return NOT_TAG_END;
    }

    // `;` is TT2's directive separator, so anything may follow it —
    // `[% END; INCLUDE footer %]` is an END directive and an INCLUDE in one
    // tag. Past this point the scanner stops judging and hands the tag's
    // contents to `block_end`, which is the rule that actually knows how to
    // read them.
    //
    // That hand-off is the point. Deciding form by form what may follow `END`
    // meant duplicating `block_end`'s job in C, and the two drifted apart twice
    // — first over comments, then over `;` itself. The separator is the natural
    // seam: before it the scanner only has to recognise the keyword, after it
    // the grammar owns everything.
    if (lexer->lookahead == ';') {
      advance_content(lexer);
      return scan_to_tag_close(lexer);
    }

    if (lexer->lookahead == '#') {
      // A comment runs to the end of the line, or to the delimiter if that
      // comes first — the same rule the grammar's `eol_comment` follows.
      advance_content(lexer);
      for (;;) {
        if (lexer->eof(lexer)) return NOT_TAG_END;
        if (lexer->lookahead == '\n') {
          advance_content(lexer);
          break;
        }
        if (lexer->lookahead == '%') {
          advance_content(lexer);
          if (lexer->lookahead == ']') return TAG_END;
          continue;
        }
        advance_content(lexer);
      }
      continue;
    }

    if (is_chomp_flag(lexer->lookahead)) {
      advance_content(lexer);
      if (lexer->lookahead != '%') return NOT_TAG_END;
      advance_content(lexer);
      return lexer->lookahead == ']' ? TAG_END : NOT_TAG_END;
    }

    if (lexer->lookahead == '%') {
      advance_content(lexer);
      return lexer->lookahead == ']' ? TAG_END : NOT_TAG_END;
    }

    return NOT_TAG_END;
  }
}

// Called with `[` as the current lookahead. It consumes enough of the candidate
// to decide whether the ordinary grammar should receive it as `block_end`.
// Every consumed character is also marked as raw text; those marks are ignored
// when this is a real END tag, because returning false rewinds the lexer to the
// start of the external token.
static enum TagEnding scan_tag_candidate(TSLexer *lexer) {
  advance_content(lexer);
  if (lexer->lookahead != '%') return NOT_TAG_END;

  advance_content(lexer);
  if (is_chomp_flag(lexer->lookahead)) advance_content(lexer);
  while (is_space(lexer->lookahead)) advance_content(lexer);

  if (lexer->lookahead != 'E') return NOT_TAG_END;
  advance_content(lexer);
  if (lexer->lookahead != 'N') return NOT_TAG_END;
  advance_content(lexer);
  if (lexer->lookahead != 'D') return NOT_TAG_END;
  advance_content(lexer);

  // `ENDS` is a variable, not the keyword.
  if (is_word_char(lexer->lookahead)) return NOT_TAG_END;
  return at_tag_ending(lexer);
}

void *tree_sitter_template_toolkit_external_scanner_create(void) {
  return NULL;
}

void tree_sitter_template_toolkit_external_scanner_destroy(void *payload) {
  (void)payload;
}

unsigned tree_sitter_template_toolkit_external_scanner_serialize(
    void *payload, char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}

void tree_sitter_template_toolkit_external_scanner_deserialize(
    void *payload, const char *buffer, unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}

bool tree_sitter_template_toolkit_external_scanner_scan(
    void *payload, TSLexer *lexer, const bool *valid_symbols) {
  (void)payload;

  // In error recovery every external token is valid at once. Scanning then
  // would consume the rest of the file as Perl on the strength of a mistake
  // made somewhere else entirely — a broken string, say. Stand down instead and
  // let the ordinary rules recover.
  if (valid_symbols[ERROR_SENTINEL]) return false;

  if (!valid_symbols[RAW_CONTENT_FRAGMENT]) return false;

  lexer->result_symbol = RAW_CONTENT_FRAGMENT;

  // Start empty. `mark_end` is what actually fixes the token's end, so the
  // lookahead below can read past it and be rewound.
  lexer->mark_end(lexer);
  if (lexer->eof(lexer)) return false;

  // Stop before every ambiguous bracket. The visible `raw_content` rule joins
  // the resulting hidden fragments, and the next scan begins exactly at `[`,
  // where one-character lookahead is safe.
  if (lexer->lookahead != '[') {
    do {
      advance_content(lexer);
    } while (!lexer->eof(lexer) && lexer->lookahead != '[');
    return true;
  }

  // A real closing tag belongs to `block_end`, not to the raw token. Returning
  // false rewinds all lookahead above. Every other result consumed at least one
  // byte of Perl source and can be emitted as the next hidden fragment.
  return scan_tag_candidate(lexer) != TAG_END;
}
