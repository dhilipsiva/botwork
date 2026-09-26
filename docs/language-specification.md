# Core Language Specification

Version 1 defines the target core semantics for the interpreter refactor. **Some rules are not implemented yet.** The evidence table below records those gaps; [language behavior](language.md) describes the working prototype. This specification fixes expected results before implementation and does not claim complete language conformance or the 9.5+ quality target.

## Evaluation

**E1 — Program execution.** Parse and validate the whole file before executing statements in source order. Syntax errors and invalid control placement prevent all execution. Stop at the first uncaught runtime error. Definitions become available when their defining statement executes; they are not hoisted.

**E2 — Expressions.** Evaluate an ordinary binary operator's left operand before its right operand; stop at the first evaluation error. Apply the operator after both values are available. Evaluate array elements and map values in source order. Evaluate an assignment's entire right-hand side before updating its destination. Parentheses change grouping, not operand evaluation order.

**E3 — Calls.** Resolve the complete statement signature before evaluating arguments. Evaluate arguments once, left to right, in the caller's environment. Only after all arguments succeed, create the invocation frame and bind its parameters. A failed argument must not install any parameter bindings. Completed argument effects are not rolled back. Native statements follow the same argument-order contract.

**E4 — Boolean selection.** Conditions require booleans; numbers, strings, collections, and `None` have no implicit truthiness. For `and`/`or`, evaluate and require a boolean left operand first. `false and rhs` yields `false`; `true or rhs` yields `true`, without evaluating or checking the type of `rhs`. Otherwise evaluate `rhs` and require a boolean. Both branches must still be syntactically valid.

## Values and Operators

**V1 — Value kinds.** Values are `None`, boolean, signed 32-bit integer, finite 32-bit float, Unicode string, ordered array, or map with string keys. An absent variable is an error, distinct from a variable bound to `None`. `None` represents no returned value; no source literal for it is currently defined. Strings follow the [decoding rules](language.md#strings). Arithmetic follows the [checked numeric contract](language.md#arithmetic-boundaries-and-errors); the full signed literal range must be supported.

**V2 — Grouping.** Binary precedence, weakest first: `or`; `and`; `==`/`!=`; `<`/`<=`/`>`/`>=`; `+`/`-`; `*`/`/`/`%`; `^`. Each level associates left except power, which associates right. Comparisons are ordinary binary operations, not mathematical chains: `1 < 2 < 3` attempts to compare a boolean with an integer and fails. Unary `-` and `!` bind above multiplication and below a power to their right. Negative exponents and repeated prefixes are allowed: `-2 ^ 2` is `-4`, `2 ^ -2 ^ 2` is `0.0625`, and `!!true` is `true`. [Power rules](language.md#powers-and-unary-operators) include checked intermediate results.

## Bindings and Definitions

**S1 — Lexical lookup.** Each run has its own outer frame. Each custom invocation gets fresh parameter and local bindings. Read a variable or statement from the current frame, then the defining environment of the currently executing custom statement and its lexical ancestors. Script-level lookup uses the run frame. Bindings are read when used, not snapshotted when a definition is declared. A caller's unrelated private locals are not visible to its callee. Nested statement definitions belong to the defining invocation and disappear when that invocation finishes; statements are not first-class returned values.

**S2 — Assignment and lifetime.** Assignment writes the current invocation or run frame, shadowing an outer variable without changing it. `If`, `While`, and `Try/Catch` bodies share their enclosing frame. Discard invocation locals on normal completion, return, or error; recursive calls get distinct frames. Completed changes in the frame where `Try` executes remain after a caught error.

**S3 — Loops.** `For` evaluates its array once, then visits its values in order. Its iterator is a temporary binding in the current frame. On completion, break, return, or error, restore that frame's previous binding, or remove the temporary binding if none existed there; an inherited binding becomes visible again. Empty iteration leaves bindings unchanged. Other body assignments use the enclosing frame. `While` evaluates its boolean condition before every iteration. `Continue` starts the next iteration; `Break` exits the nearest loop in the same invocation.

## Completion and Return Values

**C1 — Control transfer.** Normal completion, `Return`, `Break`, `Continue`, and evaluation failure are distinct outcomes. Propagate a return through every nested block to its invocation boundary. Consume loop controls only at their nearest enclosing loop. A callee cannot break or continue its caller's loop. `Return` requires a custom-statement body; `Break`/`Continue` require a loop in that same body or at script level. Reject invalid placement during validation, including inside unused definitions.

**C2 — Results.** `Return |value|` returns exactly that evaluated value. Bare `Return` and normal custom-statement completion return `None`; trailing or unreachable statements cannot change the result. Assignment returns its assigned value; native `Log` returns its logged value. Definitions and normally completed control constructs return `None`. Blocks and loops do not accumulate implicit result arrays. No pending control outcome may survive an invocation boundary.

## Errors and Recovery

**F1 — Catchable failures.** Undefined variables/statements, incompatible values, numeric conversion/overflow, unsupported access, and output failures are runtime errors. `Try` requires one `Catch`; its handler runs once after a body error. A handler error propagates outward. `Return`, `Break`, and `Continue` bypass handlers. An error while evaluating a return expression remains catchable because no return value has been produced yet.

**F2 — Effects and reporting.** Error handling does not undo completed assignments or output. A failed direct assignment preserves its destination; a failed output operation may already have written bytes. Syntax/validation errors cannot be caught by the script. Successfully handled errors exit with CLI status `0`; uncaught failures exit with `1` and diagnostics on stderr. Normal `Log` output uses stdout. Numeric conversion errors remain runtime errors despite the prototype's `ParsingIntegerError` name.

## Evidence and Implementation Gaps

Test names below are executable expectations, not a claim that every clause has exhaustive coverage. Run `cargo test --test language_contract` for the current contract cases and `cargo test --test regressions -- --ignored` for pending regressions. Also run release mode. Enable each pending case when its implementation TODO is complete.

| Rules | Evidence and current status |
| --- | --- |
| E1, F2 | CLI tests `syntax_failure_prevents_execution_of_the_whole_program`, `invalid_control_placement_prevents_all_cli_execution`, and `runtime_failure_preserves_prior_output_but_skips_later_statements`, plus contract `definitions_become_available_when_executed`, pass. `assembled_programs_are_validated_before_any_assignment_or_definition` verifies library entry-point validation before effects. |
| E2 | Contract tests `collections_report_the_first_source_order_error` and `catch_preserves_completed_work_and_a_failed_assignment_destination` pass. The owned AST evaluator now stops after a left-operand failure; `binary_evaluation_does_not_visit_the_right_operand_after_a_left_error` instruments visited expressions to prove the right operand is skipped. |
| E3 | `resolve_a_call_before_evaluating_its_arguments`, regression `all_arguments_are_evaluated_in_caller_scope`, and contract `a_failed_argument_does_not_bind_earlier_parameters` pass. `failed_arguments_install_no_frame_or_partial_bindings_and_stop_in_order` instruments visits and checks caller state after failure. |
| E4 | `conditions_require_booleans_without_truthiness_conversion` and regressions `false_and_does_not_evaluate_the_right_operand` and `true_or_does_not_evaluate_the_right_operand` pass. Evaluator cases `boolean_operators_skip_irrelevant_values_and_failures`, `boolean_operators_reject_the_left_type_before_visiting_the_right`, `boolean_truth_tables_evaluate_required_operands_once_in_order`, and `required_boolean_operands_preserve_errors_and_type_requirements` verify selection and error/type priority with visitation instrumentation. CLI tests reject malformed skipped syntax before execution. |
| V1, V2 | Grammar/evaluator tests and examples `01`, `03`, `04`, `05`, and `06` cover implemented values, operators, and names. `ordering_comparisons_do_not_form_mathematical_chains` passes. Regression `minimum_signed_integer_literal_is_representable` is pending. |
| S1, S2 | All lexical-lookup, invocation-local assignment/definition, and caller-preservation contract/regression cases pass. Evaluator checks cover live ancestor updates, variable/statement shadowing, direct and mutual recursion, recursive failure recovery, frame cleanup, and context-clone isolation. Example `09` checks visible scope behavior and native Log inside an invocation. |
| S3 | `for_evaluates_its_array_once`, the While regression, and `for_restores_its_binding_after_completion_break_and_error` pass. Evaluator cases cover absent/None/inherited bindings, nested iterators, empty/failed iterables, and completion, Continue, Break, Return, and errors before frame disposal. |
| C1, C2, F1 | Regressions `final_return_produces_a_scalar` and `nested_return_skips_remaining_outer_statements` pass. Contract cases cover bare return, fallthrough, exact value kinds, repeated calls, nested returns and handlers, failed return expressions, and nearest-loop controls. Evaluator tests `return_discards_unreachable_control_statements_in_the_same_loop` and `nested_for_and_while_loops_consume_only_their_own_controls` pass, alongside example `08`. AST validation tests, extracted-node/Pair checks, and CLI fixtures cover unused definitions, unreachable branches, lexical control targets, original locations, and failure before any execution. Defensive runtime boundaries also reject escaping controls. |

[Contract tests](../tests/language_contract.rs), [regressions](../tests/regressions.rs), [unit tests](../src/core/eval/tests.rs), and [CLI tests](../tests/cli.rs) provide the named evidence. Ignored cases are visible unfinished work and must not count as passing conformance.

## Deliberately Separate Contracts

The complete-value TODO still owns collection equality, mutation/aliasing, map iteration and duplicate keys, and exact numeric conversion/comparison policies. Naming normalization/collisions, full whitespace and multilingual rules, imports, resource limits, structured diagnostics, native registration, and async execution also remain separate tasks. Accepted dot access currently returns the documented temporary `UnsupportedAccessError`; implementing lookup must replace that behavior. Expand this specification and its evidence as those decisions are implemented.
