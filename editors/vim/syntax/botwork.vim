" Vim syntax file
" Language: Botwork scripts, suites (*.suite.botwork), and datasets
"           (*.dataset.botwork)
" Groups follow editors/tree-sitter-botwork/queries/highlights.scm.

if exists('b:current_syntax')
  finish
endif

" Statement words: a sentence's words outside its parameters.
syntax match botworkWords /[^|#{}\\ \t"=@][^|#{}\\"]*/ contains=botworkQualifier
syntax match botworkQualifier /\S\{-}::/ contained

" Control keywords start a statement, in any letter case. `Else`, `Catch`, and
" `Finally` follow a closing brace; `In` and `As` belong to `For` and `Import`.
syntax match botworkKeyword /\c^\s*\zs\(if\|for\|while\|try\|return\|break\|continue\|rethrow\|import\|eventually\|retry\)\>/
syntax match botworkKeyword /\c}\s*\zs\(else\|catch\|finally\)\>/
syntax match botworkKeyword /\c\({\s*\)\@<=\(if\|for\|while\|try\|return\|break\|continue\|rethrow\|eventually\|retry\)\>/
syntax match botworkKeyword /\c^\s*\zs\(else\|catch\|finally\)\>/
syntax match botworkKeyword /\c\(^\s*for\s*|[^|]*|\s*\)\@<=in\>/
syntax match botworkKeyword /\c\(^\s*import\s*|[^|]*|\s*\)\@<=as\>/

" Suites and datasets add keywords that start a declaration, or follow one's
" parameter: `Suite |"s"| Named |"S"|`, `Case |"c"| Using |"d"| As |row|`.
if expand('%:t') =~# '\.\(suite\|dataset\)\.botwork$'
  syntax match botworkSuiteKeyword /\c^\s*\zs\(suite\|library\|suitesetup\|suiteteardown\|casesetup\|caseteardown\|case\|dataset\|row\)\>/
  syntax match botworkSuiteKeyword /\c\(|\s*\)\@<=\(named\|tags\|from\|values\|using\|as\)\>/
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
