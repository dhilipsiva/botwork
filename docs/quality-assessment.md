# DSL Quality Assessment Protocol

Protocol version: 2, amended on 2026-09-30 by [roadmap decisions](decisions.md) D1 and D2; see [version 2 amendments](#version-2-amendments). Version 1's text is kept below, and the amendments take precedence where they differ. This is a preregistered assessment plan, not an assessment result. No participant sessions, independent scores, or release measurements have been collected yet.

## Scope and Scoring

Assess Botwork as a language for acceptance testing and automation using the weighted rubric in [TODO.md](../TODO.md#95-quality-target). The six weights are 25%, 20%, 15%, 15%, 15%, and 10%. For dimension scores `s` on a 0–10 scale, the final score is `sum(weight * s) / 100`.

Each of at least two independent reviewers must assess the same identified release and evidence bundle. Reviewers must not have authored the implementation they assess, must declare relevant involvement, and must submit their initial scores independently before discussing differences.

| Score | Anchor |
| --- | --- |
| 10 | All relevant acceptance criteria are evidenced; the reviewed workflows expose no material friction. |
| 9 | Criteria are evidenced with minor, documented friction that does not require recurring workarounds. |
| 8 | Recurring workarounds or gaps materially affect a representative workflow. |
| Below 8 | Explain the limitations, affected workflows, and missing evidence explicitly. |

Intermediate scores require written justification tied to specific evidence. Missing evidence is unassessed, never an automatic passing score. Apply thresholds to unrounded scores: at least 9.5 for every review, at least 9.0 in each dimension, and at least 9.5 in semantics and runtime reliability. Serious correctness defects, crashes from user input, state leaks, silent data loss, and broken compatibility promises block acceptance regardless of scores.

## Representative User Tasks

These tasks define outcomes independently of syntax choices. Create runnable starting materials and automated outcome checks as the required features arrive; freeze their revision before participant recruitment. Do not simplify outcomes to accommodate implementation gaps.

| ID | Participant prompt and starting material | Success criteria |
| --- | --- | --- |
| U1: Read | Read an unfamiliar script whose caller has `quantity = 99`. A reusable helper multiplies quantity by price for `(3, 4)`, `(2, 7)`, and `(0, 7)`. Each result is logged, followed by `high` if it is at least `13`, otherwise `low`; finally the caller logs its quantity. Predict the output and explain scope. | Values in order: `12`, `low`, `14`, `high`, `0`, `low`, `99`; correct branch and caller-scope explanations. The concrete script is fixed before sessions. |
| U2: First run | In an installed environment, write and run a script that assigns `quantity = 3`, `price = 4`, calculates their product, and logs it. Only published docs are provided. | Script produces `12`, exits successfully, and contains the requested variable calculation. |
| U3: Reuse | Define a reusable custom statement that calculates quantity times unit price. Call it with `(3, 4)` and `(2, 7)` from one caller that already has its own quantity variable. | Results are `12` and `14`; caller state is preserved; one shared definition is used. |
| U4: Compose | Reuse that custom statement to calculate `total(3, 4) + total(2, 7)` and compare it with `26`. Use the documented composition model; explicit intermediates are permitted. | Result is true, each call executes once, and the statement definition is not duplicated or manually inlined. |
| U5: Dataset | Start with a named case for the total calculation. Add dataset rows `(3, 4, 12)`, `(2, 7, 99)`, and `(0, 7, 0)`, where the last entry is the expected result. Run all rows, then select the failed row again. | Three distinguishable results in the first run: pass, fail, pass; aggregate failure status; second run selects only the failed row with its stable identity. |
| U6: Repair | Run a supplied script that imports a helper with one intentionally misspelled variable reference. Use its diagnostic to repair the intended reference. | Correct reference is restored, the expected output is obtained, and no assertions or operations are removed or bypassed. |

## Session Procedure and Thresholds

- Recruit at least ten consenting participants, with at least five automation newcomers and five experienced practitioners. Declare cohort definitions and prior Botwork exposure before sessions; implementers and study authors are excluded.
- Use the same release, operating environment, starter files, and published documentation revision. Verify installation before timing U2. Participants can consult docs but receive no coaching or generated solutions; record all assistance and interruptions. Any solution-bearing hint or generated answer makes the attempt assisted and excludes it from unaided successes while retaining it in the denominator.
- Supply the reusable helper for U4 and starting case for U5 independently, so failure in an earlier task does not prevent attempting a later task. Counterbalance task order where dependencies permit.
- Start timing when each prompt and materials are revealed. Stop on independently checked success, abandonment, or the fixed 20-minute task limit. Retain incomplete attempts and timeouts in the denominator.
- Require at least 90% unaided success for **each task**, not an average across tasks. Publish counts and rates separately for each cohort. U2 must also have a median completion time of at most ten minutes across all participants; count unsuccessful U2 attempts as the task limit for this calculation. At least 80% of all participants must pass U6 within five minutes.
- Record task success, duration, observed misunderstandings, participant feedback, and assistance. Keep participant identifiers pseudonymous and avoid storing unrelated personal data.
- Revise repeated sources of confusion and repeat failed tasks with fresh participants. Retain original results and disclose changes to the release, docs, prompts, or materials. Successful retries do not erase initial findings.

These thresholds are project goals for a small formative study. Report cohort sizes and limitations alongside results; do not present them as population-wide estimates.

## Representative Automation Workflows

Assess these complete workflows as their roadmap dependencies arrive. Each must include runnable source, setup/teardown instructions, expected results, and automated checks for both success and failure. [Reference workflows](reference-workflows.md) names each one's sources and checks.

| ID | Workflow | Required evidence |
| --- | --- | --- |
| W1 | Local HTTP contract service with JSON request/response assertions | Passing and failing payloads, timeout, request diagnostics, and service cleanup. |
| W2 | Filesystem and process task in a temporary workspace | Arguments with spaces/Unicode, captured output, failed process status, and cleanup. |
| W3 | Imported custom statements shared by two scripts | Caller isolation, repeated invocation, missing import, cycle diagnostics, and source locations. |
| W4 | Dataset suite with setup and teardown | Independent row results, selection/rerun, failed setup, teardown failure, and consistent reports/status. |
| W5 | Browser acceptance against a local controlled page | Successful interaction, failing assertion, timeout/cancellation, artifacts, and session cleanup. |
| W6 | Diagnose and repair a failing imported statement | Expected/actual values, useful source/call information, consistent CLI/editor diagnostics, and verified repair. |

## Evidence and Review Record

Identify the source commit, executable build, toolchain, OS, dependency versions, adapter configuration, and protocols for every measured run. Preserve raw outcomes in addition to summaries. The final bundle must include conformance/traceability, coverage, mutation and fuzz campaigns, platform/adapter results, resource/performance measurements, usability records, and compatibility evidence required by TODO.md.

For each reviewer, record all six dimension scores, their weighted total, evidence links, blockers, minor limitations, and remediation requests. Do not average away a failing reviewer or category. Resolve material gaps and reassess the affected areas against the same criteria, retaining earlier assessments.

Protocol changes require a version increment, a reason, and an explanation of which comparisons need repeating. Freeze benchmark budgets and platform matrices before collecting performance evidence in the separate milestone-1 task; this document does not claim those measurements exist.

## Version 2 amendments

These amendments record the owner's decisions of 2026-09-30. See
[D1](decisions.md#d1-reviewers) and [D2](decisions.md#d2-usability-sessions)
for the rationale.

- **Reviewers.** The two independent scored reviews are AI review sessions.
  - Each starts fresh, with no access to authoring transcripts or earlier
    reviews.
  - Each scores the identified release from its evidence bundle alone and
    submits its scores before any comparison.
  - Each records its model and version.
  - The release evidence report states, beside the scores, that reviewers from
    one model family can share blind spots.
- **Usability sessions.** The human sessions and their thresholds are
  unchanged. The owner recruits the participants and runs the sessions from a
  prepared study kit: starter files, prompts, automated success checks, a
  timing and scoring sheet, a consent note, and a results template. The kit is
  in `usability/`, and [running the usability study](usability-study.md) is
  the facilitator's guide.
- **Pilot.** Before the sessions, an AI-simulated pilot attempts U1–U6 from the
  published documentation only. It finds confusing documentation and syntax.
  Its results are [reported separately](usability-pilot.md) and never count
  toward the thresholds.

