//! One statement, `Greet |name|`, for Botwork's WebAssembly adapter.
wit_bindgen::generate!({ path: "../../../wit/botwork.wit", world: "module" });

use botwork::statements::types::{Failure, FailureKind, Node, Value};

struct Statements;

impl exports::botwork::statements::statements::Guest for Statements {
    fn headers() -> Vec<String> {
        vec!["Greet |name|".into()]
    }

    fn call(index: u32, arguments: Vec<Value>) -> Result<Value, Failure> {
        // A value is a list of nodes, the value itself first.
        match (index, arguments[0].first()) {
            (0, Some(Node::Text(name))) => Ok(vec![Node::Text(format!("Hello, {name}"))]),
            _ => Err(Failure {
                kind: FailureKind::Error,
                message: "Greet takes text".into(),
            }),
        }
    }
}

export!(Statements);
