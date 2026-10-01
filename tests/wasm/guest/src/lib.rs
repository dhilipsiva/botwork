//! The statements Botwork's WASM tests call: values both ways, values the host
//! must refuse, failures, traps, limits, and capabilities the host denies.
wit_bindgen::generate!({ path: "../../../wit/botwork.wit", world: "module" });

use botwork::statements::types::{Failure, FailureKind, Node, Value};
use std::{collections::VecDeque, sync::atomic::{AtomicI32, Ordering}};

struct Statements;

const HEADERS: &[&str] = &[
    "Echo |value|",
    "Make |kind|",
    "Fail with |message|",
    "Insist |condition|",
    "Panic with |message|",
    "Spin",
    "Burn |rounds|",
    "Grow |mebibytes|",
    "Count",
    "Clock",
    "Read file |path|",
    "Env |name|",
    "Connect |address|",
    "Print |text|",
    "Append |items|",
];

static COUNT: AtomicI32 = AtomicI32::new(0);

/// A value as the guest works with it.
enum Item {
    None,
    Int(i32),
    Float(f32),
    Bool(bool),
    Text(String),
    Array(Vec<Item>),
    Map(Vec<(String, Item)>),
}

fn decode(nodes: &[Node], index: usize) -> Item {
    match &nodes[index] {
        Node::None => Item::None,
        Node::Int(number) => Item::Int(*number),
        Node::Float(number) => Item::Float(*number),
        Node::Boolean(flag) => Item::Bool(*flag),
        Node::Text(text) => Item::Text(text.clone()),
        Node::Array(items) => Item::Array(items.iter().map(|&item| decode(nodes, item as usize)).collect()),
        Node::Map(entries) => Item::Map(
            entries.iter().map(|(key, item)| (key.clone(), decode(nodes, *item as usize))).collect(),
        ),
    }
}

/// Lay a value out breadth first, unlike the host, so the host must accept any
/// valid tree.
fn encode(item: Item) -> Value {
    let mut nodes = vec![Node::None];
    let mut queue = VecDeque::from([(0usize, item)]);
    while let Some((index, item)) = queue.pop_front() {
        nodes[index] = match item {
            Item::None => Node::None,
            Item::Int(number) => Node::Int(number),
            Item::Float(number) => Node::Float(number),
            Item::Bool(flag) => Node::Boolean(flag),
            Item::Text(text) => Node::Text(text),
            Item::Array(items) => {
                let mut children = Vec::with_capacity(items.len());
                for item in items {
                    children.push(nodes.len() as u32);
                    queue.push_back((nodes.len(), item));
                    nodes.push(Node::None);
                }
                Node::Array(children)
            }
            Item::Map(entries) => {
                let mut children = Vec::with_capacity(entries.len());
                for (key, item) in entries {
                    children.push((key, nodes.len() as u32));
                    queue.push_back((nodes.len(), item));
                    nodes.push(Node::None);
                }
                Node::Map(children)
            }
        };
    }
    nodes
}

fn text(value: &Value) -> String {
    match decode(value, 0) {
        Item::Text(text) => text,
        _ => panic!("expected text"),
    }
}

fn int(value: &Value) -> i32 {
    match decode(value, 0) {
        Item::Int(number) => number,
        _ => panic!("expected an int"),
    }
}

fn error(message: impl ToString) -> Failure {
    Failure { kind: FailureKind::Error, message: message.to_string() }
}

/// Values the host must refuse, and two it must accept.
fn make(kind: &str) -> Value {
    match kind {
        "empty" => vec![],
        "cycle" => vec![Node::Array(vec![0])],
        "shared" => vec![Node::Array(vec![1, 1]), Node::Int(1)],
        "dangling" => vec![Node::Array(vec![5])],
        "orphan" => vec![Node::Int(1), Node::Int(2)],
        "nan" => vec![Node::Float(f32::NAN)],
        "infinity" => vec![Node::Float(f32::INFINITY)],
        "duplicate" => vec![
            Node::Map(vec![("a".into(), 1), ("a".into(), 2)]),
            Node::Int(1),
            Node::Int(2),
        ],
        "deep" => {
            let mut nodes: Vec<Node> = (1..200).map(|child| Node::Array(vec![child])).collect();
            nodes.push(Node::Int(0));
            nodes
        }
        "wide" => {
            let mut nodes = vec![Node::Array((1..=70_000).collect())];
            nodes.extend((1..=70_000).map(Node::Int));
            nodes
        }
        "long" => vec![Node::Text("x".repeat(2 * 1024 * 1024))],
        // Depth-first, as the host lays values out.
        "depth-first" => vec![
            Node::Map(vec![("b".into(), 1), ("a".into(), 3)]),
            Node::Array(vec![2]),
            Node::Text("deep".into()),
            Node::Boolean(true),
        ],
        other => panic!("no value named {other}"),
    }
}

impl exports::botwork::statements::statements::Guest for Statements {
    fn headers() -> Vec<String> {
        HEADERS.iter().map(|header| header.to_string()).collect()
    }

    fn call(index: u32, arguments: Vec<Value>) -> Result<Value, Failure> {
        let item = match HEADERS[index as usize] {
            "Echo |value|" => decode(&arguments[0], 0),
            "Make |kind|" => return Ok(make(&text(&arguments[0]))),
            "Fail with |message|" => return Err(error(text(&arguments[0]))),
            "Insist |condition|" => match decode(&arguments[0], 0) {
                Item::Bool(true) => Item::Bool(true),
                _ => {
                    return Err(Failure {
                        kind: FailureKind::Assertion,
                        message: "the condition does not hold".into(),
                    })
                }
            },
            "Panic with |message|" => panic!("{}", text(&arguments[0])),
            "Spin" => loop {
                std::hint::black_box(());
            },
            "Burn |rounds|" => {
                let mut total = 0i32;
                for round in 0..int(&arguments[0]) {
                    total = std::hint::black_box(total.wrapping_add(round));
                }
                Item::Int(total)
            }
            "Grow |mebibytes|" => {
                let bytes = int(&arguments[0]) as usize * 1024 * 1024;
                let block = std::hint::black_box(vec![1u8; bytes]);
                Item::Int((block.len() / (1024 * 1024)) as i32)
            }
            "Count" => Item::Int(COUNT.fetch_add(1, Ordering::Relaxed) + 1),
            "Clock" => Item::Int(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(error)?
                    .as_secs() as i32,
            ),
            "Read file |path|" => Item::Text(std::fs::read_to_string(text(&arguments[0])).map_err(error)?),
            "Env |name|" => match std::env::var(text(&arguments[0])) {
                Ok(value) => Item::Text(value),
                Err(_) => Item::None,
            },
            "Connect |address|" => {
                std::net::TcpStream::connect(text(&arguments[0])).map_err(error)?;
                Item::Bool(true)
            }
            "Append |items|" => match decode(&arguments[0], 0) {
                Item::Array(mut items) => {
                    items.push(Item::Int(1));
                    Item::Array(items)
                }
                _ => return Err(error("Append takes an array")),
            },
            "Print |text|" => {
                let printed = text(&arguments[0]);
                print!("{printed}");
                eprint!("{printed}");
                Item::Text(printed)
            }
            other => return Err(error(format!("no statement {other}"))),
        };
        Ok(encode(item))
    }
}

export!(Statements);
