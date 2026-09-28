use super::*;

pub(super) struct WireError<'a> {
    pub code: u16,
    pub fields: [&'a str; 3],
    limit: u64,
    resource: Option<&'static str>,
}
impl<'a> WireError<'a> {
    pub(super) fn read(
        protocol: &WorkerProtocol,
        input: &mut Decoder<'a>,
    ) -> DiagnosticResult<Self> {
        let code = u16::from_le_bytes(input.take(2)?.try_into().unwrap());
        let count = match code {
            1003 | 2003 | 3004 | 6003 => 3,
            1001 | 1002 | 1004 | 2001 | 2002 | 2004 | 3001 | 3002 | 3003 | 4001 | 4002 | 4003
            | 5001 | 5002 | 5003 | 6001 | 6002 | 7001 | 7002 | 7003 | 8001 | 9001 | 9002 => 1,
            _ => return Err(invalid("Unknown worker diagnostic code")),
        };
        let mut fields = [""; 3];
        for field in fields.iter_mut().take(count) {
            *field = input.string()?;
        }
        let (limit, resource) = if code == 8001 {
            (input.u64()?, Some(protocol.resource(fields[0])?))
        } else {
            (0, None)
        };
        Ok(Self {
            code,
            fields,
            limit,
            resource,
        })
    }
    pub(super) fn summary(&self) -> (BWErr, usize) {
        let fields = self.fields.map(prefix);
        let omitted = self.fields.iter().filter(|field| field.len() > 256).count();
        let resource = self.resource.map(|name| {
            if name.len() > 256 {
                "<resource identifier truncated>"
            } else {
                name
            }
        });
        (
            Self {
                fields,
                resource,
                ..*self
            }
            .build(),
            omitted,
        )
    }
    pub(super) fn build(&self) -> BWErr {
        match self.code {
            1001 => BWErr::ParsingError(self.fields[0].into()),
            1002 => BWErr::ControlFlowError(self.fields[0].into()),
            1004 => BWErr::SignatureError(self.fields[0].into()),
            2001 => BWErr::VariableNotDefined(self.fields[0].into()),
            2002 => BWErr::StatementNotDefined(self.fields[0].into()),
            2004 => BWErr::ParameterMissingError(self.fields[0].into()),
            3001 => BWErr::ParsingIntegerError(self.fields[0].into()),
            3002 => BWErr::ArithmeticError(self.fields[0].into()),
            3003 => BWErr::OperationIncompatibleError(self.fields[0].into()),
            4001 => BWErr::OutputError(self.fields[0].into()),
            4002 => BWErr::NativeError(self.fields[0].into()),
            4003 => BWErr::NativePanic(self.fields[0].into()),
            5001 => BWErr::Cancelled(self.fields[0].into()),
            5002 => BWErr::Timeout(self.fields[0].into()),
            5003 => BWErr::AsyncRuntime(self.fields[0].into()),
            6001 => BWErr::ImportRead(self.fields[0].into()),
            6002 => BWErr::ImportCycle(self.fields[0].into()),
            7001 => BWErr::InputError(self.fields[0].into()),
            7002 => BWErr::RunConfiguration(self.fields[0].into()),
            7003 => BWErr::SourceRead(self.fields[0].into()),
            9001 => BWErr::AssertionFailed(self.fields[0].into()),
            9002 => BWErr::ExplicitFailure(self.fields[0].into()),
            1003 => BWErr::DuplicateParameter {
                name: self.fields[0].into(),
                original: self.fields[1].into(),
                duplicate: self.fields[2].into(),
            },
            2003 => BWErr::DuplicateStatement {
                signature: self.fields[0].into(),
                original: self.fields[1].into(),
                duplicate: self.fields[2].into(),
            },
            3004 => BWErr::CollectionAccessError {
                path: self.fields[0].into(),
                segment: self.fields[1].into(),
                reason: self.fields[2].into(),
            },
            6003 => BWErr::DuplicateNamespace {
                namespace: self.fields[0].into(),
                original: self.fields[1].into(),
                duplicate: self.fields[2].into(),
            },
            8001 => BWErr::ResourceLimit {
                resource: self.resource.expect("admitted resource"),
                limit: self.limit,
            },
            _ => unreachable!("admitted diagnostic code"),
        }
    }
}

pub(super) fn encode(
    protocol: &WorkerProtocol,
    output: &mut Encoder<'_>,
    error: &BWErr,
) -> DiagnosticResult<()> {
    let code: u16 = error.code().as_str()[2..]
        .parse()
        .expect("stable diagnostic code");
    output.put(&code.to_le_bytes())?;
    match error {
        BWErr::ParsingError(text)
        | BWErr::ControlFlowError(text)
        | BWErr::SignatureError(text)
        | BWErr::VariableNotDefined(text)
        | BWErr::StatementNotDefined(text)
        | BWErr::ParameterMissingError(text)
        | BWErr::ParsingIntegerError(text)
        | BWErr::ArithmeticError(text)
        | BWErr::OperationIncompatibleError(text)
        | BWErr::OutputError(text)
        | BWErr::NativeError(text)
        | BWErr::NativePanic(text)
        | BWErr::Cancelled(text)
        | BWErr::Timeout(text)
        | BWErr::AsyncRuntime(text)
        | BWErr::ImportRead(text)
        | BWErr::ImportCycle(text)
        | BWErr::InputError(text)
        | BWErr::RunConfiguration(text)
        | BWErr::AssertionFailed(text)
        | BWErr::ExplicitFailure(text)
        | BWErr::SourceRead(text) => output.string(text),
        BWErr::DuplicateParameter {
            name,
            original,
            duplicate,
        } => {
            output.string(name)?;
            output.string(original)?;
            output.string(duplicate)?;
            Ok(())
        }
        BWErr::DuplicateStatement {
            signature,
            original,
            duplicate,
        } => {
            output.string(signature)?;
            output.string(original)?;
            output.string(duplicate)?;
            Ok(())
        }
        BWErr::CollectionAccessError {
            path,
            segment,
            reason,
        } => {
            output.string(path)?;
            output.string(segment)?;
            output.string(reason)?;
            Ok(())
        }
        BWErr::DuplicateNamespace {
            namespace,
            original,
            duplicate,
        } => {
            output.string(namespace)?;
            output.string(original)?;
            output.string(duplicate)?;
            Ok(())
        }
        BWErr::ResourceLimit { resource, limit } => {
            protocol.resource(resource)?;
            output.string(resource)?;
            output.u64(*limit)
        }
    }
}
