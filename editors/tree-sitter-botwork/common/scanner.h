// External scanner shared by the botwork and botwork_suite grammars.
//
// It recognizes what regular expressions cannot express as the interpreter does:
// - comments: `###` opens a block comment that must close with the next `###`,
//   and any other `#` starts a line comment;
// - line breaks: a statement boundary where one may end a statement, and
//   otherwise whitespace, except inside a call's sentence, where a line break
//   may only precede the closing `}`;
// - sentence words: the first word of a call or definition, which cannot be a
//   reserved control keyword, and later words, which can be anything;
// - control keywords, matched case-insensitively as whole words.
#include "tree_sitter/parser.h"

#include <stdbool.h>
#include <stdint.h>
#include <string.h>

enum TokenType {
  COMMENT,
  NEWLINE,
  SOFT_NEWLINE,
  START_WORD,
  PART_WORD,
  KW_IF,
  KW_IN,
  KW_ELSE,
  KW_FOR,
  KW_BREAK,
  KW_RETURN,
  KW_CONTINUE,
  KW_WHILE,
  KW_TRY,
  KW_CATCH,
  KW_FINALLY,
  KW_RETHROW,
  KW_IMPORT,
  KW_AS,
  KW_EVENTUALLY,
  KW_RETRY,
  ERROR_RECOVERY,
};

// Keywords in token order, starting at KW_IF. `in` and `as` may still start a
// sentence; the others are reserved at the start of a statement.
static const char *const KEYWORDS[] = {
    "if",    "in",    "else",    "for",    "break",  "return",     "continue", "while",
    "try",   "catch", "finally", "rethrow", "import", "as",         "eventually", "retry",
};
#define KEYWORD_COUNT (sizeof(KEYWORDS) / sizeof(KEYWORDS[0]))
#define MAX_KEYWORD 10

static inline bool is_delimiter(int32_t character) {
  switch (character) {
  case ' ': case '\t': case '\r': case '\n':
  case '|': case '{': case '}': case '#': case '\\': case 0:
    return true;
  default:
    return false;
  }
}

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }

static bool scan_comment(TSLexer *lexer) {
  advance(lexer);  // #
  if (lexer->lookahead == '#') {
    advance(lexer);
    if (lexer->lookahead == '#') {
      advance(lexer);
      // A block comment: everything up to and including the next `###`.
      int hashes = 0;
      while (!lexer->eof(lexer)) {
        if (lexer->lookahead == '#') {
          hashes++;
          advance(lexer);
          if (hashes == 3) {
            lexer->mark_end(lexer);
            lexer->result_symbol = COMMENT;
            return true;
          }
        } else {
          hashes = 0;
          advance(lexer);
        }
      }
      return false;  // Unclosed, as the interpreter reports.
    }
  }
  while (!lexer->eof(lexer) && lexer->lookahead != '\n' && lexer->lookahead != '\r') {
    advance(lexer);
  }
  lexer->mark_end(lexer);
  lexer->result_symbol = COMMENT;
  return true;
}

/// Consume one LF or CRLF line ending; a bare CR is invalid.
static bool line_ending(TSLexer *lexer) {
  if (lexer->lookahead == '\r') {
    advance(lexer);
  }
  if (lexer->lookahead != '\n') {
    return false;
  }
  advance(lexer);
  return true;
}

static bool scan_newline(TSLexer *lexer, const bool *valid) {
  if (valid[NEWLINE]) {
    if (!line_ending(lexer)) {
      return false;
    }
    lexer->mark_end(lexer);
    lexer->result_symbol = NEWLINE;
    return true;
  }
  if (!valid[PART_WORD]) {
    if (!line_ending(lexer)) {
      return false;
    }
    lexer->mark_end(lexer);
    lexer->result_symbol = SOFT_NEWLINE;
    return true;
  }
  // Inside a call's sentence: line breaks may only precede its closing brace.
  bool any = false;
  for (;;) {
    if (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
      advance(lexer);
    } else if (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
      if (!line_ending(lexer)) {
        return false;
      }
      any = true;
      lexer->mark_end(lexer);
    } else {
      break;
    }
  }
  if (!any || lexer->lookahead != '}') {
    return false;
  }
  lexer->result_symbol = SOFT_NEWLINE;
  return true;
}

static bool scan_botwork(void *payload, TSLexer *lexer, const bool *valid) {
  (void)payload;
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, true);
  }
  if (lexer->lookahead == '#') {
    return valid[COMMENT] && scan_comment(lexer);
  }
  if (valid[ERROR_RECOVERY]) {
    return false;
  }
  if (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
    return scan_newline(lexer, valid);
  }
  bool keyword_valid = false;
  for (unsigned index = 0; index < KEYWORD_COUNT; index++) {
    keyword_valid = keyword_valid || valid[KW_IF + index];
  }
  if (!valid[START_WORD] && !valid[PART_WORD] && !keyword_valid) {
    return false;
  }
  char word[MAX_KEYWORD + 1];
  unsigned length = 0;
  bool ascii = true;
  while (!lexer->eof(lexer) && !is_delimiter(lexer->lookahead)) {
    int32_t character = lexer->lookahead;
    if (length < MAX_KEYWORD && character < 128) {
      word[length] = (char)(character >= 'A' && character <= 'Z' ? character + 32 : character);
    } else {
      ascii = false;
    }
    length++;
    advance(lexer);
  }
  if (length == 0) {
    return false;
  }
  lexer->mark_end(lexer);
  // A sentence in progress absorbs every word, keywords included.
  if (valid[PART_WORD]) {
    lexer->result_symbol = PART_WORD;
    return true;
  }
  int keyword = -1;
  if (ascii && length <= MAX_KEYWORD) {
    word[length] = 0;
    for (unsigned index = 0; index < KEYWORD_COUNT; index++) {
      if (strcmp(word, KEYWORDS[index]) == 0) {
        keyword = (int)index;
      }
    }
  }
  if (keyword >= 0 && valid[KW_IF + keyword]) {
    lexer->result_symbol = (enum TokenType)(KW_IF + keyword);
    return true;
  }
  if (keyword >= 0 && KW_IF + keyword != KW_IN && KW_IF + keyword != KW_AS) {
    return false;
  }
  if (valid[START_WORD]) {
    lexer->result_symbol = START_WORD;
    return true;
  }
  return false;
}
