//! Versioned, bounded worker frames. This is distinct from readable Log output.
#![doc = include_str!("../../../docs/worker-protocol.md")]

use crate::core::{
    diagnostic::{Diagnostic, DiagnosticLimits, DiagnosticResult, DiagnosticSize},
    grammar::{BWErr, Literal},
    operation::OperationControl,
    value_limits::{ValueLimits, ValueSize},
};
use std::io::{Read, Write};

mod catalog;
mod diagnostics;
mod values;

pub const VERSION: u16 = 1;
const MAGIC: &[u8; 4] = b"BWIP";
const HEADER: usize = 15;

#[derive(Clone, Debug)]
pub struct ProtocolLimits {
    pub frame_bytes: usize,
    pub arguments: usize,
    pub argument_nodes: usize,
    pub argument_payload_bytes: usize,
    pub sources: usize,
    pub in_flight_bytes: usize,
    pub values: ValueLimits,
    pub diagnostics: DiagnosticLimits,
}

impl Default for ProtocolLimits {
    fn default() -> Self {
        Self {
            frame_bytes: 1024 * 1024,
            arguments: 1024,
            argument_nodes: 262_144,
            argument_payload_bytes: 8 * 1024 * 1024,
            sources: 1024,
            in_flight_bytes: 32 * 1024 * 1024,
            values: ValueLimits::default(),
            diagnostics: DiagnosticLimits::default(),
        }
    }
}

/// Extra static names are an explicit host catalog, never leaked from wire input.
#[derive(Clone, Debug, Default)]
pub struct WorkerProtocol {
    pub limits: ProtocolLimits,
    pub resource_names: &'static [&'static str],
    pub labels: &'static [&'static str],
}

impl WorkerProtocol {
    pub(crate) fn validate(&self) -> DiagnosticResult<()> {
        self.limits.values.validate()?;
        self.limits.diagnostics.validate()?;
        Ok(())
    }
    fn resource(&self, name: &str) -> DiagnosticResult<&'static str> {
        catalog::RESOURCES
            .iter()
            .chain(self.resource_names)
            .copied()
            .find(|known| *known == name)
            .ok_or_else(|| {
                invalid("Unknown static resource name; configure the worker protocol catalog")
            })
    }
    fn label(&self, name: &str) -> DiagnosticResult<&'static str> {
        ["source", "expression"]
            .into_iter()
            .chain(self.labels.iter().copied())
            .find(|known| *known == name)
            .ok_or_else(|| {
                invalid("Unknown static diagnostic label; configure the worker protocol catalog")
            })
    }

    pub fn encode_request(&self, values: &[Literal]) -> DiagnosticResult<Vec<u8>> {
        self.request_plan(values, &OperationControl::default())?
            .encode()
    }
    pub fn decode_request(&self, bytes: &[u8]) -> DiagnosticResult<Vec<Literal>> {
        self.validate()?;
        let control = OperationControl::default();
        let mut reader = self.reader(bytes, 0, &control)?;
        let count = reader.count(self.limits.arguments, "worker protocol arguments")?;
        let mut nodes = 0;
        let mut payload = 0;
        for _ in 0..count {
            let size = values::scan(&mut reader, &self.limits.values, 1)?;
            add(
                &mut nodes,
                size.nodes,
                self.limits.argument_nodes,
                "worker protocol argument nodes",
            )?;
            add(
                &mut payload,
                size.payload_bytes,
                self.limits.argument_payload_bytes,
                "worker protocol argument payload bytes",
            )?;
        }
        reader.end()?;
        let mut reader = self.reader(bytes, 0, &control)?;
        reader.u64()?;
        Ok((0..count).map(|_| values::build(&mut reader)).collect())
    }
    pub fn encode_response(
        &self,
        result: Result<&Literal, &Diagnostic>,
    ) -> DiagnosticResult<Vec<u8>> {
        let control = OperationControl::default();
        self.validate()?;
        match result {
            Ok(value) => {
                self.limits.values.check(value)?;
                crate::core::grammar::validate_numeric_values(value)
                    .map_err(|_| invalid("Worker values must be finite"))?;
                let mut writer = Encoder::counter(self.limits.frame_bytes, &control)?;
                values::encode(&mut writer, value)?;
                let mut writer = writer.buffer(1);
                values::encode(&mut writer, value)?;
                writer.finish()
            }
            Err(error) => diagnostics::encode(self, error, &control),
        }
    }
    pub fn decode_response(&self, bytes: &[u8]) -> DiagnosticResult<Result<Literal, Diagnostic>> {
        Ok(
            match self.response_plan(bytes, &OperationControl::default())? {
                ResponsePlan::Value { bytes, .. } => {
                    let control = OperationControl::default();
                    let mut reader = self.reader(bytes, 1, &control)?;
                    Ok(values::build(&mut reader))
                }
                ResponsePlan::Error(plan) => Err(plan.build()),
            },
        )
    }
    /// Reads exactly one frame plus EOF; oversized bodies are rejected before allocation.
    pub fn read_request(&self, input: &mut impl Read) -> DiagnosticResult<Vec<Literal>> {
        let frame = self.read_frame(input)?;
        self.decode_request(&frame)
    }
    pub fn write_response(
        &self,
        output: &mut impl Write,
        result: Result<&Literal, &Diagnostic>,
    ) -> DiagnosticResult<()> {
        let frame = self.encode_response(result)?;
        output
            .write_all(&frame)
            .and_then(|()| output.flush())
            .map_err(io_error)
    }
    /// Serve one request through EOF, validating both sides of a native signature.
    /// A typed callback error is a successfully delivered response, not an I/O error.
    pub fn serve_once(
        &self,
        signature: &crate::core::signature::StatementSignature,
        input: &mut impl Read,
        output: &mut impl Write,
        callback: impl FnOnce(Vec<Literal>) -> DiagnosticResult<Literal>,
    ) -> DiagnosticResult<()> {
        use crate::core::{diagnostic::OwnedDiagnostic, value_limits::Owned};
        let values = Owned::new(self.read_request(input)?);
        let checked = (|| {
            if signature.origin() != crate::core::signature::StatementOrigin::Native {
                return Err(invalid("Worker signature must describe a native operation"));
            }
            if values.len() != signature.parameters().len() {
                return Err(Diagnostic::formatted(
                    BWErr::ParameterMissingError,
                    format_args!("Worker request has the wrong argument count"),
                ));
            }
            for (index, value) in values.iter().enumerate() {
                signature.validate_argument(index, value, |message| {
                    Diagnostic::formatted(BWErr::OperationIncompatibleError, message)
                })?;
            }
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback(values.into_inner())
            })) {
                Ok(result) => result,
                Err(_) => Err(Diagnostic::formatted(
                    BWErr::NativePanic,
                    format_args!("{}", signature.normalized()),
                )),
            }
        })();
        match checked {
            Ok(value) => {
                let value = Owned::new(value);
                match signature.validate_return(&value, |message| {
                    Diagnostic::formatted(BWErr::OperationIncompatibleError, message)
                }) {
                    Ok(()) => self.write_response(output, Ok(&value)),
                    Err(error) => self.write_response(output, Err(&error)),
                }
            }
            Err(error) => {
                let error = OwnedDiagnostic::new(error);
                self.write_response(output, Err(error.as_ref()))
            }
        }
    }
    fn read_frame(&self, input: &mut impl Read) -> DiagnosticResult<Vec<u8>> {
        self.validate()?;
        if self.limits.frame_bytes < HEADER {
            return Err(limit(
                "worker protocol frame bytes",
                self.limits.frame_bytes,
            ));
        }
        let mut header = [0; HEADER];
        input.read_exact(&mut header).map_err(io_error)?;
        validate_header(&header)?;
        if header[6] != 0 {
            return Err(invalid("Unexpected worker request kind"));
        }
        let length = usize::try_from(u64::from_le_bytes(header[7..15].try_into().unwrap()))
            .map_err(|_| invalid("Worker frame length is not representable"))?;
        let total = HEADER
            .checked_add(length)
            .filter(|total| *total <= self.limits.frame_bytes)
            .ok_or_else(|| limit("worker protocol frame bytes", self.limits.frame_bytes))?;
        let mut bytes = Vec::with_capacity(total);
        bytes.extend_from_slice(&header);
        bytes.resize(total, 0);
        input.read_exact(&mut bytes[HEADER..]).map_err(io_error)?;
        let mut extra = [0];
        loop {
            match input.read(&mut extra) {
                Ok(0) => break,
                Ok(_) => return Err(invalid("Trailing data after worker frame")),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(bytes)
    }
    pub(crate) fn request_plan<'a>(
        &'a self,
        values: &'a [Literal],
        control: &'a OperationControl,
    ) -> DiagnosticResult<RequestPlan<'a>> {
        control.checkpoint()?;
        self.validate()?;
        if values.len() > self.limits.arguments {
            return Err(limit("worker protocol arguments", self.limits.arguments));
        }
        let mut nodes = 0;
        let mut payload = 0;
        for value in values {
            control.checkpoint()?;
            let size = self.limits.values.check(value)?;
            crate::core::grammar::validate_numeric_values(value)
                .map_err(|_| invalid("Worker values must be finite"))?;
            add(
                &mut nodes,
                size.nodes,
                self.limits.argument_nodes,
                "worker protocol argument nodes",
            )?;
            add(
                &mut payload,
                size.payload_bytes,
                self.limits.argument_payload_bytes,
                "worker protocol argument payload bytes",
            )?;
        }
        let mut writer = Encoder::counter(self.limits.frame_bytes, control)?;
        writer.u64(values.len() as u64)?;
        for value in values {
            values::encode(&mut writer, value)?;
        }
        Ok(RequestPlan { values, writer })
    }
    fn reader<'a>(
        &self,
        bytes: &'a [u8],
        kind: u8,
        control: &'a OperationControl,
    ) -> DiagnosticResult<Decoder<'a>> {
        control.checkpoint()?;
        if bytes.len() > self.limits.frame_bytes {
            return Err(limit(
                "worker protocol frame bytes",
                self.limits.frame_bytes,
            ));
        }
        validate_header(bytes)?;
        if bytes[6] != kind {
            return Err(invalid("Unexpected worker frame kind"));
        }
        let length = u64::from_le_bytes(bytes[7..15].try_into().unwrap());
        if length != (bytes.len() - HEADER) as u64 {
            return Err(invalid(
                "Worker frame is truncated or contains trailing data",
            ));
        }
        Ok(Decoder {
            bytes: &bytes[HEADER..],
            offset: 0,
            control,
        })
    }
    pub(crate) fn response_plan<'a>(
        &'a self,
        bytes: &'a [u8],
        control: &'a OperationControl,
    ) -> DiagnosticResult<ResponsePlan<'a>> {
        control.checkpoint()?;
        self.validate()?;
        if bytes.len() > self.limits.frame_bytes {
            return Err(limit(
                "worker protocol frame bytes",
                self.limits.frame_bytes,
            ));
        }
        validate_header(bytes)?;
        match bytes[6] {
            1 => {
                let mut reader = self.reader(bytes, 1, control)?;
                let size = values::scan(&mut reader, &self.limits.values, 1)?;
                reader.end()?;
                Ok(ResponsePlan::Value {
                    bytes,
                    size,
                    kind: crate::core::signature::ValueKind::ALL[bytes[HEADER] as usize],
                })
            }
            2 => diagnostics::plan(self, self.reader(bytes, 2, control)?)
                .map(Box::new)
                .map(ResponsePlan::Error),
            _ => Err(invalid("Unexpected worker response kind")),
        }
    }
    pub(crate) fn build_value(&self, bytes: &[u8]) -> Literal {
        let control = OperationControl::default();
        values::build(
            &mut self
                .reader(bytes, 1, &control)
                .expect("admitted worker frame"),
        )
    }
}

pub(crate) struct RequestPlan<'a> {
    values: &'a [Literal],
    writer: Encoder<'a>,
}
impl RequestPlan<'_> {
    pub(crate) fn bytes(&self) -> usize {
        self.writer.size
    }
    pub(crate) fn encode(self) -> DiagnosticResult<Vec<u8>> {
        let mut writer = self.writer.buffer(0);
        writer.u64(self.values.len() as u64)?;
        for value in self.values {
            values::encode(&mut writer, value)?;
        }
        writer.finish()
    }
}

pub(crate) enum ResponsePlan<'a> {
    Value {
        bytes: &'a [u8],
        size: ValueSize,
        kind: crate::core::signature::ValueKind,
    },
    Error(Box<diagnostics::DiagnosticPlan<'a>>),
}

fn validate_header(bytes: &[u8]) -> DiagnosticResult<()> {
    if bytes.len() < HEADER || &bytes[..4] != MAGIC {
        return Err(invalid("Invalid worker frame header"));
    }
    if u16::from_le_bytes(bytes[4..6].try_into().unwrap()) != VERSION {
        return Err(invalid("Unsupported worker protocol version"));
    }
    Ok(())
}
fn invalid(reason: &str) -> Diagnostic {
    Diagnostic::formatted(
        BWErr::AsyncRuntime,
        format_args!("Worker protocol: {reason}"),
    )
}
fn io_error(error: std::io::Error) -> Diagnostic {
    Diagnostic::formatted(
        BWErr::AsyncRuntime,
        format_args!("Worker protocol I/O failed: {error}"),
    )
}
fn limit(resource: &'static str, limit: usize) -> Diagnostic {
    BWErr::ResourceLimit {
        resource,
        limit: limit as u64,
    }
    .into()
}
fn add(
    value: &mut usize,
    amount: usize,
    maximum: usize,
    resource: &'static str,
) -> DiagnosticResult<()> {
    *value = value
        .checked_add(amount)
        .filter(|next| *next <= maximum)
        .ok_or_else(|| limit(resource, maximum))?;
    Ok(())
}

struct Encoder<'a> {
    bytes: Option<Vec<u8>>,
    size: usize,
    maximum: usize,
    expected: usize,
    control: &'a OperationControl,
}
impl<'a> Encoder<'a> {
    fn counter(maximum: usize, control: &'a OperationControl) -> DiagnosticResult<Self> {
        if maximum < HEADER {
            return Err(limit("worker protocol frame bytes", maximum));
        }
        Ok(Self {
            bytes: None,
            size: HEADER,
            maximum,
            expected: 0,
            control,
        })
    }
    fn buffer(self, kind: u8) -> Self {
        let mut bytes = Vec::with_capacity(self.size);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.push(kind);
        bytes.extend_from_slice(&((self.size - HEADER) as u64).to_le_bytes());
        Self {
            bytes: Some(bytes),
            size: HEADER,
            expected: self.size,
            ..self
        }
    }
    fn put(&mut self, bytes: &[u8]) -> DiagnosticResult<()> {
        self.control.checkpoint()?;
        add(
            &mut self.size,
            bytes.len(),
            self.maximum,
            "worker protocol frame bytes",
        )?;
        if let Some(output) = &mut self.bytes {
            output.extend_from_slice(bytes);
        }
        Ok(())
    }
    fn byte(&mut self, value: u8) -> DiagnosticResult<()> {
        self.put(&[value])
    }
    fn u64(&mut self, value: u64) -> DiagnosticResult<()> {
        self.put(&value.to_le_bytes())
    }
    fn string(&mut self, text: &str) -> DiagnosticResult<()> {
        self.u64(text.len() as u64)?;
        self.put(text.as_bytes())
    }
    fn finish(self) -> DiagnosticResult<Vec<u8>> {
        self.control.checkpoint()?;
        if self.size != self.expected {
            return Err(invalid("Worker frame changed after admission"));
        }
        Ok(self.bytes.expect("encoded frame"))
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
    control: &'a OperationControl,
}
impl<'a> Decoder<'a> {
    fn take(&mut self, count: usize) -> DiagnosticResult<&'a [u8]> {
        self.control.checkpoint()?;
        let end = self
            .offset
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| invalid("Truncated worker payload"))?;
        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }
    fn byte(&mut self) -> DiagnosticResult<u8> {
        Ok(self.take(1)?[0])
    }
    fn u64(&mut self) -> DiagnosticResult<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn usize(&mut self) -> DiagnosticResult<usize> {
        usize::try_from(self.u64()?).map_err(|_| invalid("Worker count is not representable"))
    }
    fn count(&mut self, maximum: usize, resource: &'static str) -> DiagnosticResult<usize> {
        let count = self.usize()?;
        if count > maximum {
            return Err(limit(resource, maximum));
        }
        if count > self.bytes.len() - self.offset {
            return Err(invalid("Impossible worker record count"));
        }
        Ok(count)
    }
    fn boolean(&mut self) -> DiagnosticResult<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid("Invalid worker boolean")),
        }
    }
    fn string(&mut self) -> DiagnosticResult<&'a str> {
        let count = self.usize()?;
        std::str::from_utf8(self.take(count)?).map_err(|_| invalid("Invalid UTF-8 in worker frame"))
    }
    fn end(&self) -> DiagnosticResult<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid("Trailing worker payload"))
        }
    }
}

#[cfg(test)]
mod tests;
