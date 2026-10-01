//! Values between Botwork and WebDriver's JSON: handles, selectors, script
//! arguments and results, and screenshots' base64.
use super::*;
use serde_json::{Map, Value};

/// The key W3C WebDriver names an element reference with.
pub(super) const ELEMENT: &str = "element-6066-11e4-a52e-4f735466cecf";
/// The deepest JSON a script result may nest.
const MAX_DEPTH: usize = 64;

/// An element handle: the session it belongs to and the driver's reference.
pub(super) fn element(session: &str, id: &str) -> Literal {
    Literal::Map(HashMap::from([
        ("session".to_owned(), Literal::String(session.to_owned())),
        ("element".to_owned(), Literal::String(id.to_owned())),
    ]))
}

/// The session a handle names and, for an element handle, its reference.
pub(super) fn handle(value: &Literal, what: &str) -> Result<(String, Option<String>), String> {
    let Literal::Map(map) = value else {
        return Err(format!(
            "{what} must be a handle Map, not a {}",
            value.kind().as_str()
        ));
    };
    let Some(Literal::String(session)) = map.get("session") else {
        return Err(format!(
            "{what} has no `session`; pass a value Open Browser or Find Element returned"
        ));
    };
    let element = match map.get("element") {
        None => None,
        Some(Literal::String(element)) => Some(element.clone()),
        Some(other) => {
            return Err(format!(
                "{what}'s `element` must be a String, not a {}",
                other.kind().as_str()
            ))
        }
    };
    Ok((session.clone(), element))
}

/// A selector: a String is CSS; a Map names one strategy.
pub(super) fn selector(value: &Literal) -> Result<(&'static str, String), String> {
    const STRATEGIES: [(&str, &str); 5] = [
        ("css", "css selector"),
        ("xpath", "xpath"),
        ("link_text", "link text"),
        ("partial_link_text", "partial link text"),
        ("tag_name", "tag name"),
    ];
    match value {
        Literal::String(css) => Ok(("css selector", css.clone())),
        Literal::Map(map) if map.len() == 1 => {
            let (key, value) = map.iter().next().expect("one entry");
            let using = STRATEGIES
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, using)| *using)
                .ok_or_else(|| {
                    format!("Unknown selector strategy `{key}`; use css, xpath, link_text, partial_link_text, or tag_name")
                })?;
            match value {
                Literal::String(text) => Ok((using, text.clone())),
                other => Err(format!(
                    "The `{key}` selector must be a String, not a {}",
                    other.kind().as_str()
                )),
            }
        }
        Literal::Map(_) => {
            Err("A selector Map names exactly one strategy, such as {xpath: \"//h1\"}".into())
        }
        other => Err(format!(
            "A selector is a String or a Map, not a {}",
            other.kind().as_str()
        )),
    }
}

/// How a selector reads in messages.
pub(super) fn describe(using: &str, value: &str) -> String {
    format!("{using} `{value}`")
}

/// A Botwork value as JSON for the driver; element handles become references.
pub(super) fn to_json(value: &Literal) -> Result<Value, String> {
    Ok(match value {
        Literal::None => Value::Null,
        Literal::Bool(value) => Value::Bool(*value),
        Literal::Int(value) => Value::from(*value),
        Literal::Float(value) => serde_json::Number::from_f64(f64::from(*value))
            .map(Value::Number)
            .ok_or_else(|| format!("The Float {value} has no JSON form"))?,
        Literal::String(value) => Value::String(value.clone()),
        Literal::Array(items) => Value::Array(items.iter().map(to_json).collect::<Result<_, _>>()?),
        Literal::Map(map) => {
            if let (Some(Literal::String(_)), Some(Literal::String(element)), 2) =
                (map.get("session"), map.get("element"), map.len())
            {
                return Ok(Value::Object(Map::from_iter([(
                    ELEMENT.to_owned(),
                    Value::String(element.clone()),
                )])));
            }
            let mut object = Map::new();
            for (key, value) in map {
                object.insert(key.clone(), to_json(value)?);
            }
            Value::Object(object)
        }
    })
}

/// The driver's JSON as a Botwork value: element references become handles of
/// `session`, and numbers keep their exact value or fail.
pub(super) fn from_json(value: &Value, session: &str) -> Result<Literal, String> {
    convert(value, session, 0)
}

fn convert(value: &Value, session: &str, depth: usize) -> Result<Literal, String> {
    if depth > MAX_DEPTH {
        return Err(format!("the result nests deeper than {MAX_DEPTH} levels"));
    }
    Ok(match value {
        Value::Null => Literal::None,
        Value::Bool(value) => Literal::Bool(*value),
        Value::Number(number) => number_value(number)?,
        Value::String(text) => Literal::String(text.clone()),
        Value::Array(items) => Literal::Array(
            items
                .iter()
                .map(|item| convert(item, session, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(object) => {
            if let (Some(Value::String(id)), 1) = (object.get(ELEMENT), object.len()) {
                return Ok(element(session, id));
            }
            let mut map = HashMap::with_capacity(object.len());
            for (key, value) in object {
                map.insert(key.clone(), convert(value, session, depth + 1)?);
            }
            Literal::Map(map)
        }
    })
}

/// An Int when the number is a whole 32-bit integer, otherwise a Float, unless
/// a whole number would round in one.
fn number_value(number: &serde_json::Number) -> Result<Literal, String> {
    if let Some(whole) = number.as_i64() {
        if let Ok(int) = i32::try_from(whole) {
            return Ok(Literal::Int(int));
        }
    }
    let float = number
        .as_f64()
        .ok_or_else(|| format!("the number {number} has no Botwork value"))?;
    let single = float as f32;
    if float.fract() == 0.0 && f64::from(single) != float {
        return Err(format!(
            "the whole number {number} is beyond 32 bits, and a 32-bit Float would round it"
        ));
    }
    Ok(Literal::Float(single))
}

/// Decode standard base64, as screenshots arrive.
pub(super) fn base64(text: &str) -> Result<Vec<u8>, String> {
    fn digit(byte: u8) -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let bytes: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let body = bytes
        .strip_suffix(b"==")
        .or_else(|| bytes.strip_suffix(b"="))
        .unwrap_or(&bytes);
    if !bytes.len().is_multiple_of(4) || body.len() % 4 == 1 {
        return Err("the screenshot is not valid base64".into());
    }
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 2);
    for chunk in body.chunks(4) {
        let mut buffer = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            let value = digit(*byte).ok_or("the screenshot is not valid base64")?;
            buffer |= value << (18 - 6 * index);
        }
        let produced = chunk.len() * 6 / 8;
        out.extend_from_slice(&buffer.to_be_bytes()[1..1 + produced]);
    }
    Ok(out)
}
