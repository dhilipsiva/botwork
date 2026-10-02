# Reference workflows

The six representative workflows of the [quality assessment](quality-assessment.md#representative-automation-workflows),
each as runnable source with its setup, teardown, expected results, and the
automated checks of its success and its failures. Usability sessions and the
release assessment use these workflows; `tests/reference_workflows.rs` checks
that every source and check this page names exists.

| ID | Workflow | Sources |
| --- | --- | --- |
| W1 | HTTP contract test | `examples/36-http-contract.botwork` |
| W2 | Filesystem and process task | `examples/35-build-catalogue.botwork` |
| W3 | Imported custom statements shared by scripts | `examples/modules/arithmetic.botwork` |
| W4 | Dataset suite with setup and teardown | `examples/26-fixtures.suite.botwork` |
| W5 | Browser acceptance against a local page | `examples/browser/acceptance.suite.botwork` |
| W6 | Diagnose and repair a failing imported statement | `usability/tasks/U6/main.botwork` |

## W1: HTTP contract test

- **Sources:** `examples/36-http-contract.botwork`, `examples/modules/http-contract.botwork`, `examples/inputs/http-contract.json`
- **Setup and teardown:** a local HTTP service answers the contract; the test starts and stops it. See [complete automation examples](automation-examples.md#check-an-http-response-contract).
- **Expected:** the script logs `HTTP contract passed (200)`.
- **Success:** `http_contract_uses_imported_helper_and_bundled_json_inputs`
- **Failure:** `http_contract_rejects_status_header_and_body_mismatches`, `http_contract_enforces_the_configured_deadline`

## W2: Filesystem and process task

- **Sources:** `examples/35-build-catalogue.botwork`, `examples/modules/catalogue.botwork`, `examples/inputs/catalogue.json`, `examples/inputs/catalogue.txt`
- **Setup and teardown:** the script stages its work in a temporary directory, which `Finally` removes on every path. See [complete automation examples](automation-examples.md#build-a-verified-catalogue).
- **Expected:** a verified catalogue at the destination, and no staging directory left.
- **Success:** `builds_verified_artifact_using_bundled_configuration_and_relative_paths`, `accepts_unicode_crlf_and_literal_paths_through_variable_overrides`
- **Failure:** `mismatch_does_not_publish_and_still_cleans_staging`, `nonzero_process_exit_fails_before_publication_and_cleans_staging`, `process_launch_failure_also_cleans_staging`

## W3: Imported custom statements shared by scripts

- **Sources:** `examples/modules/arithmetic.botwork`, imported by `examples/19-local-imports.botwork`, `examples/23-named-cases.suite.botwork`, and `examples/24-parameterized-cases.suite.botwork`
- **Setup and teardown:** none.
- **Expected:** each importer gets its own module state; see [modules](extending.md#botwork-modules).
- **Success:** `local_import_example_checks_namespaces_results_and_collision_recovery`, `module_globals_and_helpers_are_isolated_from_the_importing_caller`, `modules_import_what_they_use_and_never_see_their_importers_imports`
- **Failure:** `cycle_construction_preserves_chain_and_all_parent_sites_through_unwinding`, `import_read_construction_admits_message_call_and_originating_site_together`, `namespace_collision_admits_both_locations_before_loading_and_preserves_original_namespace`

## W4: Dataset suite with setup and teardown

- **Sources:** `examples/26-fixtures.suite.botwork`, `examples/24-parameterized-cases.suite.botwork`, `examples/27-setup-failure.suite.botwork`
- **Setup and teardown:** suite and case fixtures; see [fixtures](fixtures.md).
- **Expected:** each row has its own outcome, and `--failures` then `--rerun-failed` reruns only the failed rows.
- **Success:** `owners_keep_distinct_state_and_each_row_receives_case_hooks`, `each_row_has_an_outcome_and_only_failed_rows_rerun_after_reordering_and_repair`
- **Failure:** `failed_case_setup_skips_its_body_and_preserves_both_failures`, `failed_suite_setup_cleans_partial_state_and_marks_unentered_cases_skipped`, `suite_teardown_failure_reruns_all_affected_selected_cases_after_repair`

## W5: Browser acceptance against a local page

- **Sources:** `examples/browser/acceptance.suite.botwork`, `examples/browser/sign-in.botwork`, `examples/browser/page.botwork`, `examples/browser/inputs.json`
- **Setup and teardown:** Chrome and its chromedriver; each row opens a browser of its own, and the run's end closes it and captures it when the row failed. See [browser and device examples](../examples/browser/README.md).
- **Expected:** each row logs its greeting and passes; with `--var 'expected_greeting="Hi"'`, each row fails with BW9001 and leaves a screenshot, the page source, and the driver's log in `artifacts/`.
- **Success:** `the_acceptance_suite_signs_in_each_person`
- **Failure:** `a_failing_acceptance_row_is_captured_recorded_and_rerun`, `chrome_covers_every_failure_path`, `fakes_capture_and_close_cancelled_runs`

## W6: Diagnose and repair a failing imported statement

- **Sources:** `usability/tasks/U6/main.botwork`, `usability/tasks/U6/pricing.botwork`, `usability/tasks/U6/prompt.md`
- **Setup and teardown:** none; the [usability study](usability-study.md) gives the task to participants.
- **Expected:** the run fails with the misspelled reference's diagnostic, pointing into `pricing.botwork`; once repaired, it prints `10`.
- **Success:** `reference_solutions_pass_their_checks`
- **Failure:** `untouched_starter_files_fail_their_checks`, `plausible_wrong_answers_fail_for_the_stated_reason`, `diagnostics_follow_edits_and_clear_on_close`
