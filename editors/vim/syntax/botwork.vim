" Vim syntax file
" Language: Botwork scripts, suites (*.suite.botwork), and datasets
"           (*.dataset.botwork)
" Groups follow editors/tree-sitter-botwork/queries/highlights.scm.

if exists('b:current_syntax')
  finish
endif

" Block comments can span many lines, so highlight from the start of the file.
syntax sync fromstart

" Statement words: a sentence's words outside its parameters.
syntax match botworkWords /[^|#{}\\ \t"=@][^|#{}\\"]*/ contains=botworkQualifier
syntax match botworkQualifier /\S\{-}::/ contained

" Control keywords start a statement, in any letter case, and end where the
" grammar's control_keyword does: before a space, tab, `|`, `{`, `}`, `#`, `\`,
" or the end of the line. `Else`, `Catch`, and
" `Finally` follow a closing brace; `In` and `As` belong to `For` and `Import`.
syntax match botworkKeyword /\c^\s*\zs\(if\|for\|while\|try\|return\|break\|continue\|rethrow\|import\|eventually\|retry\)\%([ \t|{}#\\]\|$\)\@=/
syntax match botworkKeyword /\c}\s*\zs\(else\|catch\|finally\)\%([ \t|{}#\\]\|$\)\@=/
syntax match botworkKeyword /\c\({\s*\)\@<=\(if\|for\|while\|try\|return\|break\|continue\|rethrow\|eventually\|retry\)\%([ \t|{}#\\]\|$\)\@=/
syntax match botworkKeyword /\c^\s*\zs\(else\|catch\|finally\)\%([ \t|{}#\\]\|$\)\@=/
syntax match botworkKeyword /\c\(\<else\s\+\)\@<=if\%([ \t|{}#\\]\|$\)\@=/
syntax match botworkKeyword /\c\(^\s*for\s*|[^|]*|\s*\)\@<=in\%([ \t|{}#\\]\|$\)\@=/
syntax match botworkKeyword /\c\(^\s*import\s*|[^|]*|\s*\)\@<=as\%([ \t|{}#\\]\|$\)\@=/

" Suites and datasets add keywords. A suite, case, or dataset header runs from
" its keyword to its block and may continue across lines, as in
" `Case |"c"| Named |"C"|` followed by `Using |"d"| As |row| {`; its keywords
" can appear anywhere in it. A dataset row is a header that ends with its line.
" Other declarations start a line.
if expand('%:t') =~# '\.\(suite\|dataset\)\.botwork$'
  syntax region botworkSuiteHeader matchgroup=botworkSuiteKeyword start=/\c^\s*\zs\(suite\|case\)\>/ end=/{/me=s-1 contains=botworkParameter,botworkHeaderKeyword,botworkComment,botworkBlockComment,botworkContinuation
  syntax region botworkSuiteHeader matchgroup=botworkSuiteKeyword start=/\c^\s*\zsdataset\>/ end=/{/me=s-1 end=/$/ contains=botworkParameter,botworkHeaderKeyword,botworkComment,botworkBlockComment,botworkContinuation
  syntax region botworkSuiteHeader matchgroup=botworkSuiteKeyword start=/\c^\s*\zsrow\>/ end=/$/ contains=botworkParameter,botworkHeaderKeyword,botworkComment,botworkBlockComment,botworkContinuation
  syntax match botworkHeaderKeyword /\c\<\(named\|tags\|using\|as\|from\|values\)\>/ contained
  syntax match botworkSuiteKeyword /\c^\s*\zs\(library\|suitesetup\|suiteteardown\|casesetup\|caseteardown\)\>/
endif

" Comments: `###` blocks, then `#` to the end of the line.
syntax region botworkBlockComment start=/###/ end=/###/ contains=@Spell
syntax match botworkComment /#\(##\)\@!.*$/ contains=@Spell

" A backslash at the end of a line continues the statement.
syntax match botworkContinuation /\\\s*$/

" Parameters hold expressions; a call expression holds a statement.
syntax region botworkParameter matchgroup=botworkDelimiter start=/|/ end=/|/ contains=@botworkExpression
syntax region botworkCall matchgroup=botworkBracket start=/@{/ end=/}/ contained contains=botworkWords,botworkParameter,botworkContinuation

syntax cluster botworkExpression contains=botworkCall,botworkString,botworkNumber,botworkBoolean,botworkOperator,botworkWordOperator,botworkVariable,botworkList,botworkMap
syntax region botworkString start=/"/ skip=/\\\\\|\\"/ end=/"/ contained contains=botworkEscape
syntax match botworkEscape /\\["\\n]/ contained
syntax match botworkNumber /\<\d\+\(\.\d\+\)\?\>/ contained
syntax keyword botworkBoolean true false contained
syntax keyword botworkWordOperator and or contained
syntax match botworkOperator /==\|!=\|<=\|>=\|[<>+\-*\/%^!]/ contained
syntax match botworkVariable /\<\h\w*\>/ contained
syntax region botworkList matchgroup=botworkBracket start=/\[/ end=/\]/ contained contains=@botworkExpression
syntax region botworkMap matchgroup=botworkBracket start=/{/ end=/}/ contained contains=@botworkExpression

highlight default link botworkWords Function
highlight default link botworkQualifier Include
highlight default link botworkKeyword Keyword
highlight default link botworkSuiteKeyword Keyword
highlight default link botworkHeaderKeyword Keyword
highlight default link botworkComment Comment
highlight default link botworkBlockComment Comment
highlight default link botworkContinuation Special
highlight default link botworkDelimiter Delimiter
highlight default link botworkBracket Delimiter
highlight default link botworkString String
highlight default link botworkEscape SpecialChar
highlight default link botworkNumber Number
highlight default link botworkBoolean Boolean
highlight default link botworkWordOperator Operator
highlight default link botworkOperator Operator
highlight default link botworkVariable Identifier

let b:current_syntax = 'botwork'
