use super::*;

#[derive(Clone, Debug, Default)]
pub(super) struct Fixtures {
    suite_setup: Option<Block>,
    suite_teardown: Option<Block>,
    case_setup: Option<Block>,
    case_teardown: Option<Block>,
}

/// Setup includes the suite's Library. Teardown uses the same retained context.
/// Case programs initialize their own Library and invocation/module state.
#[non_exhaustive]
pub struct FixturePrograms {
    pub setup: Program,
    pub teardown: Program,
}

impl Fixtures {
    pub(super) fn lower(
        &mut self,
        pair: Pair<Rule>,
        source: &Arc<SourceFile>,
    ) -> DiagnosticResult<()> {
        let span = Span::of(&pair, source);
        let mut inner = pair.into_inner();
        let kind = inner.next().expect("fixture kind");
        let slot = match kind.as_rule() {
            Rule::suite_setup => &mut self.suite_setup,
            Rule::suite_teardown => &mut self.suite_teardown,
            Rule::case_setup => &mut self.case_setup,
            Rule::case_teardown => &mut self.case_teardown,
            _ => unreachable!("fixture grammar"),
        };
        if slot.is_some() {
            return Err(
                configuration(format!("Duplicate fixture declaration {}", kind.as_str())).at(&span),
            );
        }
        *slot = Some(
            block(inner.next().expect("fixture body"), source)
                .map_err(|error| error.at(Some(&span)).default_diagnostic())?,
        );
        Ok(())
    }

    pub(super) fn statements(&self) -> impl Iterator<Item = &[Statement]> {
        [
            &self.suite_setup,
            &self.suite_teardown,
            &self.case_setup,
            &self.case_teardown,
        ]
        .into_iter()
        .filter_map(Option::as_ref)
        .map(|block| block.statements.as_slice())
    }

    pub(super) fn case_statements(&self, library: &[Statement], body: &Block) -> Vec<Statement> {
        let statements: Vec<_> = library
            .iter()
            .chain(self.case_setup.iter().flat_map(|setup| &setup.statements))
            .chain(&body.statements)
            .cloned()
            .collect();
        match &self.case_teardown {
            Some(cleanup) => vec![Statement {
                span: body.span.clone(),
                kind: StatementKind::Finally {
                    body: Block {
                        span: body.span.clone(),
                        statements,
                    },
                    cleanup: cleanup.clone(),
                },
            }],
            None => statements,
        }
    }
}

impl Suite {
    pub fn has_suite_fixture(&self) -> bool {
        self.fixtures.suite_setup.is_some() || self.fixtures.suite_teardown.is_some()
    }

    /// Project suite-owned phases. Execute with evaluate_suite_fixture_async so
    /// teardown is awaited even when setup or its body fails cooperatively.
    pub fn fixture_programs(&self) -> FixturePrograms {
        let program = |statements| Program {
            source: Arc::clone(&self.source),
            statements,
        };
        FixturePrograms {
            setup: program(
                self.library
                    .iter()
                    .chain(
                        self.fixtures
                            .suite_setup
                            .iter()
                            .flat_map(|setup| &setup.statements),
                    )
                    .cloned()
                    .collect(),
            ),
            teardown: program(
                self.fixtures
                    .suite_teardown
                    .iter()
                    .flat_map(|cleanup| &cleanup.statements)
                    .cloned()
                    .collect(),
            ),
        }
    }
}
