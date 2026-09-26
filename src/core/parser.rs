// Keep generated parser code separate from handwritten runtime coverage.
use pest_derive::Parser;

#[derive(Parser)]
#[grammar = "src/core/grammar.pest"]
pub struct BWParser;
