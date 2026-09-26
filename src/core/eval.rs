use pest::iterators::Pair;
use pest::Parser;
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{self, Write},
    rc::Rc,
};

use super::grammar::{BWErr, BWParser, Literal, LiteralResult, Operate, Rule, PRATT_PARSER};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, PartialEq)]
enum InteruptKind {
    #[default]
    None,
    Continue,
    Break,
    Return,
}

/// Metadata used to indicate program interuption.
/// Like BREAK, CONTINUE, RETURN
#[derive(Clone, Debug, Default)]
struct Interupt {
    kind: InteruptKind,
    literal: Literal,
}

#[derive(Clone, Default)]
enum StmtType {
    #[default]
    None,
    Native(Callback),
    UserDefined {
        header: String,
        block: String,
    },
}

#[derive(Clone, Default)]
pub struct Context {
    variables: HashMap<String, Literal>,
    statements: HashMap<String, StmtType>,
    interupt: Interupt,
}

impl Context {
    fn get_variable(&self, name: &String) -> LiteralResult {
        match self.variables.get(name) {
            Some(value) => Ok(value.to_owned()),
            None => Err(BWErr::VariableNotDefined(name.clone())),
        }
    }

    /*
    fn contains_variable(&self, name: &String) -> bool {
        self.variables.contains_key(name)
    }
    */

    fn set_variable(&mut self, name: String, literal: Literal) -> Option<Literal> {
        self.variables.insert(name, literal)
    }

    fn contains_statement(&self, name: &String) -> bool {
        self.statements.contains_key(name)
    }

    fn get_statement(&self, name: &String, statement: &mut Option<StmtType>) {
        *statement = self.statements.get(name).cloned();
    }

    pub fn init_statements(&mut self) {
        self.statements
            .insert("log|param|".into(), StmtType::Native(log_param));
    }

    /// TODO: Right now, this stringifies the header & block pair and parses everytimme
    /// it is required to execute. As one might realize, this could degrade the persormance.
    /// I was not able to push Pair<Rule> into the state because it had lot of lifetime to deal with.
    /// I am new to rust and lifetimes is still a concept that I trying to grasp.
    /// It soulw be awesome if somone could help me with this.
    fn push_statement(&mut self, stmt_header: Pair<Rule>, stmt_block: Pair<Rule>) {
        let stmt_hash = get_stmt_hash(&stmt_header);
        self.statements.insert(
            stmt_hash,
            StmtType::UserDefined {
                header: stmt_header.as_str().into(),
                block: stmt_block.as_str().into(),
            },
        );
    }

    fn push_interupt(&mut self, interupt: Interupt) {
        if InteruptKind::None == self.interupt.kind {
            self.interupt = interupt
        } else {
            unreachable!("Tried pusing interupt, while an other one os already in progress!")
        }
    }

    fn push_interupt_break(&mut self) {
        self.push_interupt(Interupt {
            kind: InteruptKind::Break,
            literal: Literal::None,
        })
    }

    fn push_interupt_continue(&mut self) {
        self.push_interupt(Interupt {
            kind: InteruptKind::Continue,
            literal: Literal::None,
        })
    }

    fn push_interupt_return(&mut self, literal: Literal) {
        self.push_interupt(Interupt {
            kind: InteruptKind::Return,
            literal,
        })
    }

    fn has_loop_interupt(&self) -> bool {
        match self.interupt.kind {
            InteruptKind::Continue | InteruptKind::Break => true,
            InteruptKind::None | InteruptKind::Return => false,
        }
    }

    fn has_return_interupt(&self) -> bool {
        matches!(self.interupt.kind, InteruptKind::Return)
    }

    fn pop_interupt(&mut self) -> Interupt {
        let interupt = self.interupt.clone();
        self.interupt.literal = Literal::None; // Just in case
        self.interupt.kind = InteruptKind::None;
        interupt
    }
}

type Callback = fn(Pair<Rule>, &mut Context) -> LiteralResult;

fn hashify(text: &str) -> String {
    text.replace(' ', "").to_lowercase()
}

fn get_stmt_hash(pair: &Pair<Rule>) -> String {
    let mut hash = String::new();
    for pair in pair.clone().into_inner() {
        match pair.as_rule() {
            Rule::part => hash.push_str(&hashify(pair.as_str())),
            Rule::param_invoke | Rule::ident => hash.push_str("|param|"),
            _ => unreachable!(),
        }
    }
    hash
}

fn log_param(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let param_pair = pair
        .into_inner()
        .rfind(|pair| pair.as_rule() == Rule::param_invoke)
        .ok_or(BWErr::ParameterMissingError(
            "`Log {param}` requires atleast 1 parameter".into(),
        ))?;
    let ok = botwork(param_pair, globals)?;
    write_log(&ok, &mut io::stdout().lock())?;
    Ok(ok)
}

fn write_log(value: &Literal, output: &mut impl Write) -> Result<(), BWErr> {
    writeln!(output, "{value}").map_err(|error| BWErr::OutputError(error.to_string()))
}

fn no_op(_pair: Pair<Rule>, _globals: &mut Context) -> LiteralResult {
    Ok(Literal::None)
}

fn stmt_invoke(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let hash = get_stmt_hash(&pair);
    if !globals.contains_statement(&hash) {
        return Err(BWErr::StatementNotDefined(pair.as_str().into()));
    }
    let mut statement: Option<StmtType> = None;
    globals.get_statement(&hash, &mut statement);
    let statement = statement.ok_or(BWErr::VariableNotDefined(pair.as_str().into()))?;
    match statement {
        StmtType::None => unreachable!("None is just a default!"),
        StmtType::Native(statement) => statement(pair, globals),
        StmtType::UserDefined { header, block } => {
            //TODO: Zip parameter names from header & insert it onto globals
            let mut header = BWParser::parse(Rule::stmt_header, &header)
                .map_err(|_| BWErr::ParsingError("Parsing cached header failed".into()))?;
            let mut block = BWParser::parse(Rule::stmt_block, &block)
                .map_err(|_| BWErr::ParsingError("Parsing cached block failed".into()))?;
            let stmt_header = header
                .next()
                .ok_or(BWErr::ParsingError("No header pair found in Pairs".into()))?;
            let stmt_block = block
                .next()
                .ok_or(BWErr::ParsingError("No Block pair found in Pairs".into()))?;
            if header.count() != 0 || block.count() != 0 {
                return Err(BWErr::ParsingError(
                    "More cached header/block pair found!".into(),
                ));
            }
            let idents = stmt_header
                .into_inner()
                .filter(|pair| pair.as_rule() == Rule::ident);
            // .collect::<Vec<Pair<Rule>>>();
            let param_invoks = pair
                .into_inner()
                .filter(|pair| pair.as_rule() == Rule::param_invoke);
            // .collect::<Vec<Pair<Rule>>>();
            for (ident, param_invoke) in idents.zip(param_invoks) {
                let literal = botwork(param_invoke, globals)?;
                globals.set_variable(ident.as_str().into(), literal);
            }
            botwork(stmt_block, globals)
        }
    }
}

fn pratt_parse(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let globals = Rc::new(RefCell::new(globals));
    let result = PRATT_PARSER
        .map_primary(|primary| botwork(primary, &mut globals.borrow_mut()))
        .map_infix(|lhs, op, rhs| op.as_rule().operate_binary(lhs?, rhs?))
        .map_prefix(|op, rhs| op.as_rule().operate_unary(rhs?))
        .parse(pair.into_inner());
    result
}

fn integer(pair: Pair<Rule>, _globals: &mut Context) -> LiteralResult {
    match pair.as_str().parse() {
        Ok(integer) => Ok(Literal::Int(integer)),
        Err(err) => Err(BWErr::ParsingIntegerError(err.to_string())),
    }
}

fn float(pair: Pair<Rule>, _globals: &mut Context) -> LiteralResult {
    match pair.as_str().parse() {
        Ok(float) => Ok(Literal::Float(float)),
        Err(err) => Err(BWErr::ParsingIntegerError(err.to_string())),
    }
}

fn boolean_true(_pair: Pair<Rule>, _globals: &mut Context) -> LiteralResult {
    Ok(Literal::Bool(true))
}

fn boolean_false(_pair: Pair<Rule>, _globals: &mut Context) -> LiteralResult {
    Ok(Literal::Bool(false))
}

fn string(pair: Pair<Rule>, _globals: &mut Context) -> LiteralResult {
    let content = pair
        .into_inner()
        .next()
        .ok_or_else(|| BWErr::ParsingError("Missing string content".into()))?;
    let mut value = String::new();
    let mut chars = content.as_str().chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            value.push(match chars.next() {
                Some('n') => '\n',
                Some('"') => '"',
                Some('\\') => '\\',
                _ => return Err(BWErr::ParsingError("Invalid string escape".into())),
            });
        } else {
            value.push(character);
        }
    }
    Ok(Literal::String(value))
}

fn keyword(pair: Pair<Rule>, _globals: &mut Context) -> LiteralResult {
    Ok(Literal::String(pair.as_str().into()))
}

fn array(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut tokens = vec![];
    for inner_pair in pair.into_inner() {
        let token = botwork(inner_pair, globals)?;
        tokens.push(token);
    }
    Ok(Literal::Array(tokens))
}

fn map(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut map = HashMap::new();
    for inner_pair in pair.into_inner() {
        let mut kv = inner_pair.into_inner();
        let keyword = kv
            .next()
            .ok_or(BWErr::ParsingError("Getting keyword from map_pair".into()))?;
        let value = kv
            .next()
            .ok_or(BWErr::ParsingError("Getting keyword from map_pair".into()))?;
        if kv.next().is_some() {
            unreachable!("Map statment still has unused pairs!");
        }
        if let Literal::String(keyword) = botwork(keyword, globals)? {
            let value = botwork(value, globals)?;
            map.insert(keyword, value);
        } else {
            return Err(BWErr::ParsingError(
                "Keyword in map MUST always be a String identifier token".into(),
            ));
        }
    }
    Ok(Literal::Map(map))
}

fn stmt_assign(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut inner = pair.into_inner();
    let ident = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting ident failed".into()))?
        .as_str()
        .to_string();
    let value = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting value failed".into()))?;
    if inner.next().is_some() {
        unreachable!("Assignment statment still has unused pairs!");
    }
    let value = botwork(value, globals)?;
    globals.set_variable(ident, value.clone());
    Ok(value)
}

fn ident(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let ident = pair.as_str().to_string();
    globals.get_variable(&ident)
}

fn stmt_if(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut inner = pair.clone().into_inner();
    let condition = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting condition failed".into()))?;
    let block = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting true block failed".into()))?;
    let condition = botwork(condition, globals)?;
    if let Literal::Bool(is_true) = condition {
        if is_true {
            botwork(block, globals)
        } else {
            match inner.next() {
                Some(block) => {
                    let result = botwork(block, globals);
                    if inner.next().is_some() {
                        unreachable!("If statment still has unused pairs!");
                    }
                    result
                }
                None => no_op(pair, globals),
            }
        }
    } else {
        Err(BWErr::OperationIncompatibleError(
            "The conditional expression in `If` should always evaluvate to boolean value".into(),
        ))
    }
}

fn stmt_block(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut results = vec![];
    for block in pair.into_inner() {
        if Rule::stmt_break == block.as_rule() {
            globals.push_interupt_break();
            return Ok(Literal::Array(results));
        } else if Rule::stmt_continue == block.as_rule() {
            globals.push_interupt_continue();
            return Ok(Literal::Array(results));
        } else if globals.has_loop_interupt() {
            return Ok(Literal::Array(results));
        } else if globals.has_return_interupt() {
            let interupt = globals.pop_interupt();
            // TODO: results is lost. Think of something else.
            return Ok(interupt.literal);
        } else {
            let result = botwork(block, globals)?;
            results.push(result);
        }
    }
    Ok(Literal::Array(results))
}

fn stmt_for(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut inner = pair.into_inner();
    let ident = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting ident failed".into()))?;
    let array = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting array failed".into()))?;
    let block = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting block failed".into()))?;
    if inner.next().is_some() {
        unreachable!("For statment still has unused pairs!");
    }
    if let Literal::Array(array) = botwork(array.clone(), globals)? {
        let mut results = vec![];
        for i in array {
            globals.set_variable(ident.as_str().to_string(), i);
            let result = botwork(block.clone(), globals)?;
            results.push(result);
            let interupt = globals.pop_interupt();
            match interupt.kind {
                InteruptKind::None => (),
                InteruptKind::Continue => continue,
                InteruptKind::Break => return Ok(Literal::Array(results)),
                InteruptKind::Return => {
                    // push it back because it needs to be habdled by stmt_block
                    globals.push_interupt(interupt);
                    break;
                }
            }
        }
        Ok(Literal::Array(results))
    } else {
        Err(BWErr::OperationIncompatibleError(format!(
            "FOR loop requires array to iterate over. But found: {}",
            array
        )))
    }
}

fn stmt_while(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut inner = pair.into_inner();
    let expr = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting ident failed".into()))?;
    let block = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting block failed".into()))?;
    if inner.next().is_some() {
        unreachable!("While statment still has unused pairs!");
    }
    let mut results = vec![];
    loop {
        if let Literal::Bool(should_loop) = botwork(expr.clone(), globals)? {
            if !should_loop {
                return Ok(Literal::Array(results));
            }
            let result = botwork(block.clone(), globals)?;
            results.push(result);
            let interupt = globals.pop_interupt();
            match interupt.kind {
                InteruptKind::None => (),
                InteruptKind::Continue => continue,
                InteruptKind::Break => return Ok(Literal::Array(results)),
                InteruptKind::Return => {
                    // push it back because it needs to be habdled by stmt_block
                    globals.push_interupt(interupt);
                    return Ok(Literal::Array(results));
                }
            }
        } else {
            return Err(BWErr::OperationIncompatibleError(
                "While loop requires expressions to return boolean".into(),
            ));
        }
    }
}

fn stmt_try(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut inner = pair.into_inner();
    let try_block = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting try block failed".into()))?;
    let catch_block = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting catch block failed".into()))?;
    if inner.next().is_some() {
        unreachable!("While statment still has unused pairs!");
    }
    let try_result = botwork(try_block, globals);
    if try_result.is_ok() {
        try_result
    } else {
        botwork(catch_block, globals)
    }
}

fn stmt_define(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut inner = pair.into_inner();
    let stmt_header = inner.next().ok_or(BWErr::ParsingError(
        "Getting statement header failed".into(),
    ))?;
    let stmt_block = inner
        .next()
        .ok_or(BWErr::ParsingError("Getting statement block failed".into()))?;
    if inner.next().is_some() {
        unreachable!("Statement definition still has unused pairs!");
    }
    globals.push_statement(stmt_header, stmt_block);
    Ok(Literal::None)
}

fn stmt_return(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let mut inner = pair.into_inner();
    match inner.next() {
        Some(expr) => {
            if inner.next().is_some() {
                unreachable!("Return statement still has unused pairs!");
            }
            let literal = botwork(expr, globals)?;
            globals.push_interupt_return(literal.clone());
            Ok(literal)
        }
        None => {
            globals.push_interupt_return(Literal::None);
            Ok(Literal::None)
        }
    }
}

pub fn botwork(pair: Pair<Rule>, globals: &mut Context) -> LiteralResult {
    let op = match pair.as_rule() {
        Rule::EOI => no_op,
        Rule::stmt_invoke => stmt_invoke,
        Rule::stmt_define => stmt_define,
        Rule::stmt_assign => stmt_assign,
        Rule::param_invoke => pratt_parse,
        Rule::expression => pratt_parse,
        Rule::unary => pratt_parse,
        Rule::array => array,
        Rule::ident => ident,
        Rule::map => map,
        Rule::keyword => keyword,
        Rule::integer => integer,
        Rule::float => float,
        Rule::string => string,
        Rule::logical_not => no_op,
        Rule::boolean_true => boolean_true,
        Rule::boolean_false => boolean_false,
        Rule::stmt_if => stmt_if,
        Rule::stmt_else => stmt_block,
        Rule::stmt_for => stmt_for,
        Rule::stmt_while => stmt_while,
        Rule::stmt_try => stmt_try,
        Rule::stmt_catch => stmt_block,
        Rule::stmt_return => stmt_return,
        Rule::seperator => no_op,
        Rule::stmt_block => stmt_block,
        _ => unreachable!(),
    };
    op(pair, globals)
}
