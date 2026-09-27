use super::*;
use std::collections::HashMap;

pub(super) fn encode(output: &mut Encoder<'_>, value: &Literal) -> DiagnosticResult<()> {
    match value {
        Literal::None => output.byte(0),
        Literal::Int(value) => {
            output.byte(1)?;
            output.put(&value.to_le_bytes())
        }
        Literal::Float(value) => {
            output.byte(2)?;
            output.put(&value.to_bits().to_le_bytes())
        }
        Literal::Bool(value) => {
            output.byte(3)?;
            output.byte(u8::from(*value))
        }
        Literal::String(value) => {
            output.byte(4)?;
            output.string(value)
        }
        Literal::Array(values) => {
            output.byte(5)?;
            output.u64(values.len() as u64)?;
            for value in values {
                encode(output, value)?;
            }
            Ok(())
        }
        Literal::Map(values) => {
            output.byte(6)?;
            output.u64(values.len() as u64)?;
            if output.bytes.is_some() {
                let mut entries: Vec<_> = values.iter().collect();
                entries.sort_unstable_by_key(|(key, _)| *key);
                for (key, value) in entries {
                    output.string(key)?;
                    encode(output, value)?;
                }
            } else {
                for (key, value) in values {
                    output.string(key)?;
                    encode(output, value)?;
                }
            }
            Ok(())
        }
    }
}

pub(super) fn scan(
    input: &mut Decoder<'_>,
    limits: &ValueLimits,
    depth: usize,
) -> DiagnosticResult<ValueSize> {
    let mut size = ValueSize {
        nodes: 1,
        depth,
        payload_bytes: 0,
    };
    limits.check_size(size)?;
    match input.byte()? {
        0 => {}
        1 => {
            input.take(4)?;
            size.payload_bytes = 4;
        }
        2 => {
            let value = f32::from_bits(u32::from_le_bytes(input.take(4)?.try_into().unwrap()));
            if !value.is_finite() {
                return Err(invalid("Worker values must be finite"));
            }
            size.payload_bytes = 4;
        }
        3 => {
            input.boolean()?;
            size.payload_bytes = 1;
        }
        4 => {
            let text = input.string()?;
            limits.string_size(text.len())?;
            size.payload_bytes = text.len();
        }
        tag @ (5 | 6) => {
            let count = input.count(limits.entries, "value container entries")?;
            limits.container_header(count)?;
            let mut previous = None;
            for _ in 0..count {
                if tag == 6 {
                    let key = input.string()?;
                    limits.key_size(key.len())?;
                    if previous.is_some_and(|prior| prior >= key) {
                        return Err(invalid("Worker map keys must be unique and sorted"));
                    }
                    previous = Some(key);
                    add(
                        &mut size.payload_bytes,
                        key.len(),
                        limits.payload_bytes,
                        "value payload bytes",
                    )?;
                }
                let child = scan(input, limits, depth + 1)?;
                add(&mut size.nodes, child.nodes, limits.nodes, "value nodes")?;
                add(
                    &mut size.payload_bytes,
                    child.payload_bytes,
                    limits.payload_bytes,
                    "value payload bytes",
                )?;
                size.depth = size.depth.max(child.depth);
            }
        }
        _ => return Err(invalid("Unknown worker value tag")),
    }
    limits.check_size(size)?;
    Ok(size)
}

// Only called after scanning the entire immutable frame and reserving ownership.
// Construction uses a fresh noncancelled reader; the caller observes stop again
// after bounded construction and disposes a rejected result through Owned.
pub(super) fn build(input: &mut Decoder<'_>) -> Literal {
    match input.byte().expect("admitted tag") {
        0 => Literal::None,
        1 => Literal::Int(i32::from_le_bytes(
            input.take(4).unwrap().try_into().unwrap(),
        )),
        2 => Literal::Float(f32::from_bits(u32::from_le_bytes(
            input.take(4).unwrap().try_into().unwrap(),
        ))),
        3 => Literal::Bool(input.boolean().unwrap()),
        4 => Literal::String(input.string().unwrap().into()),
        5 => {
            let count = input.usize().unwrap();
            Literal::Array((0..count).map(|_| build(input)).collect())
        }
        6 => {
            let count = input.usize().unwrap();
            let mut values = HashMap::with_capacity(count);
            for _ in 0..count {
                let key = input.string().unwrap().to_owned();
                values.insert(key, build(input));
            }
            Literal::Map(values)
        }
        _ => unreachable!("admitted tag"),
    }
}
