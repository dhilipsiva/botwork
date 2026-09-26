use super::*;
use crate::core::{
    diagnostic::DiagnosticCode,
    signature::{StatementSignature, ValueKind},
};

#[test]
fn reservations_are_atomic_and_stops_are_latched() {
    let budget = RunBudget::new(
        RunLimits {
            imports: ImportLimits {
                loads: 2,
                source_bytes: 0,
                ..ImportLimits::default()
            },
            ..RunLimits::default()
        },
        OperationControl::default(),
    );
    let error = budget
        .charge_imports(&[(ImportResource::Loads, 1), (ImportResource::SourceBytes, 1)])
        .unwrap_err();
    assert!(error.to_string().contains("module source bytes"));
    assert_eq!(budget.import_remaining(ImportResource::Loads), 2);
    assert_eq!(
        budget.charge_imports(&[]).unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn reservations_use_checked_arithmetic_even_with_maximum_host_limits() {
    let budget = RunBudget::new(
        RunLimits {
            imports: ImportLimits {
                loads: usize::MAX,
                ..ImportLimits::default()
            },
            ..RunLimits::default()
        },
        OperationControl::default(),
    );
    budget
        .charge_imports(&[(ImportResource::Loads, usize::MAX)])
        .unwrap();
    assert_eq!(budget.import_remaining(ImportResource::Loads), 0);
    assert!(budget
        .charge_imports(&[(ImportResource::Loads, 1)])
        .is_err());
}

#[test]
fn qualified_metadata_bytes_match_owned_payload_and_preserve_contracts() {
    let mut original = StatementSignature::native("Read |café|")
        .unwrap()
        .returns(ValueKind::Int)
        .description("description")
        .documents_error(DiagnosticCode::Native, "failure")
        .unwrap();
    for namespace in ["İ", "Outer"] {
        let normalized = crate::core::ast::normalize_sentence(namespace);
        let qualified = original.qualified(namespace, &normalized);
        let display_namespace =
            qualified.display_header().len() - qualified.header().text().trim().len() - 2;
        let actual = qualified.normalized().len() * 2
            + display_namespace
            + original.normalized().len()
            + qualified.documentation().len()
            + qualified
                .parameters()
                .iter()
                .map(|p| p.name.len())
                .sum::<usize>()
            + qualified
                .documented_errors()
                .iter()
                .map(|e| e.description.len())
                .sum::<usize>();
        assert_eq!(
            original.qualified_bytes(namespace, &normalized),
            Some(actual)
        );
        assert_eq!(qualified.header(), original.header());
        assert_eq!(qualified.return_kinds(), original.return_kinds());
        original = qualified;
    }
}
