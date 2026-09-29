; Scripts; suite and dataset files add queries/suite-highlights.scm.
(comment) @comment
(string) @string
[(integer) (float) (index)] @number
[(true) (false)] @constant.builtin
(continuation) @punctuation.special

[
  "If" "Else" "For" "In" "While" "Try" "Catch" "Finally" "Return" "Break"
  "Continue" "Rethrow" "Import" "As" "Eventually" "Retry"
] @keyword

(definition_header (word) @function)
(sentence (word) @function.call)
(parameter_name (identifier) @variable.parameter)
(field_access (identifier) @property)
(pair key: (identifier) @property)
(identifier) @variable

[
  "or" "and" "==" "!=" "<" "<=" ">" ">=" "+" "-" "*" "/" "%" "^" "!" "="
] @operator

["|" "," ":" "."] @punctuation.delimiter
["{" "}" "[" "]" "(" ")" "@{"] @punctuation.bracket
