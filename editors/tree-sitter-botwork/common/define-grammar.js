/**
 * Botwork's Tree-sitter grammars, generated from one definition: `botwork` for
 * scripts and `botwork_suite` for suite and dataset files. The rules mirror
 * `src/core/grammar.pest`; `common/scanner.h` recognizes comments, sentence
 * words, and the case-insensitive control keywords.
 */

/** A case-insensitive regular expression for an ASCII keyword. */
function insensitive(word) {
  return new RegExp(
    Array.from(word)
      .map((letter) => `[${letter.toLowerCase()}${letter.toUpperCase()}]`)
      .join(''),
  );
}

/**
 * Line breaks between parts of a statement. Inside expressions and statement
 * headers, the scanner turns line breaks into whitespace; these mark the places
 * where a statement may end or continue after one.
 */
const lines = ($) => repeat($._newline);

const PREC = {
  or: 1,
  and: 2,
  equality: 3,
  comparison: 4,
  sum: 5,
  product: 6,
  unary: 7,
  power: 8,
};

// Suites and datasets, only in the botwork_suite grammar.
const SUITE_RULES = {
  suite: ($) => seq(
    alias(insensitive('suite'), 'Suite'),
    field('id', $.suite_id),
    optional($.suite_name),
    optional($.suite_tags),
    '{', lines($),
    repeat(seq($.dataset, lines($))),
    optional(seq($.library, lines($))),
    repeat(seq($.fixture, lines($))),
    repeat1(seq($.case, lines($))),
    '}',
  ),

  suite_id: ($) => seq('|', $.string, '|'),

  suite_name: ($) => seq(alias(insensitive('named'), 'Named'), $.suite_id),

  suite_tags: ($) => seq(
    alias(insensitive('tags'), 'Tags'),
    '|', '[',
    optional(seq(
      $.string,
      repeat(seq(',', $.string)),
      optional(','),
    )),
    ']', '|',
  ),

  library: ($) => seq(alias(insensitive('library'), 'Library'), $.block),

  fixture: ($) => seq(
    field('kind', choice(
      alias(insensitive('suitesetup'), 'SuiteSetup'),
      alias(insensitive('suiteteardown'), 'SuiteTeardown'),
      alias(insensitive('casesetup'), 'CaseSetup'),
      alias(insensitive('caseteardown'), 'CaseTeardown'),
    )),
    $.block,
  ),

  case: ($) => seq(
    alias(insensitive('case'), 'Case'),
    field('id', $.suite_id),
    optional($.suite_name),
    optional($.suite_tags),
    optional($.case_using),
    $.block,
  ),

  case_using: ($) => seq(
    alias(insensitive('using'), 'Using'),
    $.suite_id,
    alias($._as, 'As'),
    $.parameter_name,
  ),

  dataset: ($) => seq(
    alias(insensitive('dataset'), 'Dataset'),
    field('id', $.suite_id),
    choice(
      seq(
        alias(insensitive('from'), 'From'),
        optional(alias(choice(insensitive('json'), insensitive('csv')), $.dataset_format)),
        field('path', $.suite_id),
      ),
      seq(
        optional($.suite_name),
        optional($.suite_tags),
        '{', lines($),
        repeat1(seq($.row, lines($))),
        '}',
      ),
    ),
  ),

  row: ($) => seq(
    alias(insensitive('row'), 'Row'),
    field('id', $.suite_id),
    optional($.suite_name),
    optional($.suite_tags),
    alias(insensitive('values'), 'Values'),
    '|', $._data, '|',
  ),

  _data: ($) => choice($.data_map, $.data_array, $.string, $.data_number, $.true, $.false, $.none),

  data_number: ($) => seq(optional('-'), choice($.float, $.integer)),

  data_array: ($) => seq(
    '[',
    optional(seq(
      $._data,
      repeat(seq(',', $._data)),
      optional(','),
    )),
    ']',
  ),

  data_map: ($) => seq(
    '{',
    optional(seq(
      $.data_pair,
      repeat(seq(',', $.data_pair)),
      optional(','),
    )),
    '}',
  ),

  data_pair: ($) => seq(choice($.identifier, $.string), ':', $._data),

  none: (_) => 'none',
};

module.exports = function defineGrammar(dialect) {
  return grammar({
    name: dialect === 'suite' ? 'botwork_suite' : 'botwork',

    externals: ($) => [
      $.comment,
      $._newline,
      $._soft_newline,
      $._start_word,
      $._part_word,
      $._if,
      $._in,
      $._else,
      $._for,
      $._break,
      $._return,
      $._continue,
      $._while,
      $._try,
      $._catch,
      $._finally,
      $._rethrow,
      $._import,
      $._as,
      $._eventually,
      $._retry,
      $._error_recovery,
    ],

    extras: ($) => [/[ \t]/, $.comment, $._soft_newline],

    word: ($) => $.identifier,

    reserved: {
      global: (_) => ['true', 'false', 'and', 'or'],
    },

    // A header and a call look alike until a block follows, and a line break
    // may end a statement or precede its `Else`, `Finally`, or body.
    conflicts: ($) => [
      [$.definition_header, $.sentence],
      [$.parameter_name, $._primary],
      [$.try_statement],
      [$.if_statement],
    ],

    rules: {
      source_file: dialect === 'suite'
        ? ($) => seq(lines($), choice($.suite, $.dataset), lines($))
        : ($) => repeat($._item),

      _item: ($) => choice($._statement, $._newline),

      _statement: ($) => choice(
        $.break_statement,
        $.continue_statement,
        $.return_statement,
        $.rethrow_statement,
        $.import_statement,
        $.if_statement,
        $.for_statement,
        $.while_statement,
        $.try_statement,
        $.eventually_statement,
        $.retry_statement,
        $.definition,
        $.assignment,
        $.sentence,
      ),

      block: ($) => seq('{', repeat($._item), '}'),

      break_statement: ($) => alias($._break, 'Break'),
      continue_statement: ($) => alias($._continue, 'Continue'),
      rethrow_statement: ($) => alias($._rethrow, 'Rethrow'),
      // Like the interpreter, `Return` takes a following parameter when it can.
      return_statement: ($) => prec.right(seq(
        alias($._return, 'Return'),
        optional(seq(optional($.continuation), $.parameter)),
      )),

      import_statement: ($) => seq(
        alias($._import, 'Import'),
        '|', field('path', $.string), '|',
        alias($._as, 'As'),
        field('namespace', $.parameter_name),
      ),

      assignment: ($) => seq(
        field('name', $.parameter_name),
        '=',
        field('value', choice($.sentence, $.parameter)),
      ),

      if_statement: ($) => seq(
        alias($._if, 'If'),
        field('condition', $.parameter),
        field('consequence', $.block),
        optional(seq(lines($), $.else_clause)),
      ),

      else_clause: ($) => seq(
        alias($._else, 'Else'),
        choice($.block, $.if_statement),
      ),

      for_statement: ($) => seq(
        alias($._for, 'For'),
        field('item', $.parameter_name),
        alias($._in, 'In'),
        field('items', $.parameter),
        field('body', $.block),
      ),

      while_statement: ($) => seq(
        alias($._while, 'While'),
        field('condition', $.parameter),
        field('body', $.block),
      ),

      eventually_statement: ($) => seq(
        alias($._eventually, 'Eventually'),
        field('options', $.parameter),
        field('body', $.block),
      ),

      retry_statement: ($) => seq(
        alias($._retry, 'Retry'),
        field('options', $.parameter),
        field('body', $.block),
      ),

      try_statement: ($) => seq(
        alias($._try, 'Try'),
        field('body', $.block),
        choice(
          seq($.catch_clause, optional(seq(lines($), $.finally_clause))),
          $.finally_clause,
        ),
      ),

      catch_clause: ($) => seq(
        alias($._catch, 'Catch'),
        optional(field('binding', $.parameter_name)),
        $.block,
      ),

      finally_clause: ($) => seq(alias($._finally, 'Finally'), $.block),

      definition: ($) => seq(
        field('header', $.definition_header),
        lines($),
        field('body', $.block),
      ),

      // Sentences are greedy: a following word or parameter continues them.
      definition_header: ($) => prec.right(seq(
        alias($._start_word, $.word),
        repeat(seq(
          optional($.continuation),
          choice($.parameter_name, alias($._part_word, $.word)),
        )),
      )),

      // Sentences are greedy: a following word or parameter continues them.
      sentence: ($) => prec.right(seq(
        alias($._start_word, $.word),
        repeat(seq(
          optional($.continuation),
          choice($.parameter, alias($._part_word, $.word)),
        )),
      )),

      continuation: (_) => token(seq('\\', /[ \t]*/, /\r?\n/)),

      parameter_name: ($) => seq('|', $.identifier, '|'),

      parameter: ($) => seq('|', $._expression, '|'),

      _expression: ($) => $._operation,

      _operation: ($) => choice($.binary_expression, $._operand),

      binary_expression: ($) => {
        const table = [
          ['or', PREC.or],
          ['and', PREC.and],
          ['==', PREC.equality],
          ['!=', PREC.equality],
          ['<', PREC.comparison],
          ['<=', PREC.comparison],
          ['>', PREC.comparison],
          ['>=', PREC.comparison],
          ['+', PREC.sum],
          ['-', PREC.sum],
          ['*', PREC.product],
          ['/', PREC.product],
          ['%', PREC.product],
        ];
        return choice(...table.map(([operator, precedence]) => prec.left(precedence, seq(
          field('left', $._operation),
          field('operator', operator),
          field('right', $._operation),
        ))));
      },

      _operand: ($) => choice($.unary_expression, $.power_expression, $._primary),

      unary_expression: ($) => prec(PREC.unary, seq(
        field('operator', choice('-', '!')),
        field('operand', $._operand),
      )),

      power_expression: ($) => prec.right(PREC.power, seq(
        field('base', $._primary),
        '^',
        field('exponent', $._operand),
      )),

      _primary: ($) => choice($.primary, $.identifier, $._literal, $.call_expression, $.parenthesized_expression),

      primary: ($) => seq(
        choice($.identifier, $._literal, $.call_expression, $.parenthesized_expression),
        repeat1(choice($.field_access, $.index_access)),
      ),

      field_access: ($) => seq('.', choice($.identifier, alias(/[0-9]+/, $.index))),

      index_access: ($) => seq('[', $._expression, ']'),

      call_expression: ($) => seq('@{', $.sentence, '}'),

      parenthesized_expression: ($) => seq('(', $._expression, ')'),

      _literal: ($) => choice($.map, $.array, $.string, $.float, $.integer, $.true, $.false),

      array: ($) => seq(
        '[',
        optional(seq(
          $._expression,
          repeat(seq(',', $._expression)),
          optional(','),
        )),
        ']',
      ),

      map: ($) => seq(
        '{',
        optional(seq(
          $.pair,
          repeat(seq(',', $.pair)),
          optional(','),
        )),
        '}',
      ),

      pair: ($) => seq(
        field('key', choice($.identifier, $.string)),
        ':',
        field('value', $._expression),
      ),

      identifier: (_) => /[_\p{XID_Start}][\p{XID_Continue}]*/,
      integer: (_) => /[0-9]+/,
      float: (_) => /[0-9]+\.[0-9]+/,
      string: (_) => /"([^"\\]|\\["\\n])*"/,
      true: (_) => 'true',
      false: (_) => 'false',

      ...(dialect === 'suite' ? SUITE_RULES : {}),
    },
  });
};
