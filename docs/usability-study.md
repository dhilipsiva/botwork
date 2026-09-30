# Running the usability study

This is the facilitator's guide to the usability sessions that milestone 9
requires. The tasks, success criteria, and thresholds are registered in the
[quality assessment protocol](quality-assessment.md#representative-user-tasks).
Per [roadmap decision D2](decisions.md#d2-usability-sessions), the project owner
recruits the participants and runs the sessions with the kit in
[`usability/`](../usability/README.md). An [AI-simulated pilot](usability-pilot.md)
tried the tasks first; it never counts toward the thresholds.

## Before the study

- **Freeze the materials.** Pick one Botwork release, one documentation
  revision, and one kit revision, and use them for every session of a round.
  Each workspace records the kit's revision and file hashes in `kit.json`.
- **Prepare the environment.** Install that release so that `botwork` is on the
  participant's `PATH`, and give them an editor and the published
  documentation at that revision: `README.md`, `docs/`, and `examples/`. Leave
  out the study's own pages, which give answers away:
  `docs/quality-assessment.md`, this guide, and `docs/usability-pilot.md`.
  Nothing else: no source code, tests, reference solutions, or generated
  answers.
- **Recruit at least ten participants.** At least five must be *newcomers*, who
  have not written automated tests or automation scripts, and at least five
  *experienced*, who write them regularly in any tool. Record each cohort, and
  any earlier Botwork exposure, before the session. People who worked on
  Botwork or on this study cannot take part.

## Running a session

1. Read [the consent note](../usability/consent.md) and record the answer.
2. Create the participant's workspace, with their number from 1:

   ```sh
   python3 usability/prepare.py 7 workspace-P07
   ```

   `order.txt` lists the tasks in this participant's order. The six orders
   rotate so that each task appears early and late across participants; U2,
   the first script, always comes before the other writing tasks.
3. Check the installation with `botwork --version` before any timing.
4. For each task in order, reveal its folder and `prompt.md` and start the
   timer. Stop it when the task succeeds, when the participant stops, or at 20
   minutes. A task succeeds when the participant says they are done and its
   check passes:

   ```sh
   python3 usability/check.py U3 workspace-P07/U3
   ```

   If the check fails, say only "not yet", keep the timer running, and let the
   participant continue. Do not share the check's reasons; they would be a
   hint.
5. After U1, ask the participant to explain their answer, and score it
   (below).
6. Record one row per task in a copy of [`sheet.csv`](../usability/sheet.csv).

### Scoring U1's explanation

U1's check covers the predicted lines. The explanation is correct when the
participant says both:

- each amount is `high` when it is at least 13, and `low` otherwise, because of
  the `If` that compares it; and
- the last line is 99 because the statement's `quantity` is its own parameter:
  calling it never changes the caller's `quantity`.

### Assistance

Answer questions about the setup, never about the task. Any hint toward a
solution, or any generated answer, makes the attempt *assisted*: record
`assisted` as `yes`. The attempt still counts in the denominator but never as an
unaided success. Note every interruption.

## The scoring sheet

| Column | Values |
| --- | --- |
| `participant` | The pseudonymous code, such as `P07` |
| `cohort` | `newcomer` or `experienced` |
| `round` | `original`, or `retest` for fresh participants after changes |
| `task` | `U1` to `U6` |
| `seconds` | Time from the reveal to the end, at most 1,200 |
| `outcome` | `success`, `failure`, `abandoned`, or `timeout` |
| `assisted` | `yes` or `no` |
| `checked` | `pass` or `fail`, the last `check.py` result |
| `explanation_ok` | For U1, `yes` or `no`; empty for other tasks |
| `notes` | Misunderstandings, feedback, and interruptions |

## Scoring a round

```sh
python3 usability/score.py sheet.csv --json results.json
```

`score.py` rejects an invalid sheet with status 2. Otherwise it prints each
task's unaided successes, for all participants and for each cohort, and a
verdict for each registered threshold:

- at least ten participants, with at least five in each cohort, and every
  participant attempting every task;
- at least 90% unaided success on every task;
- a median U2 time of at most 10 minutes, counting each unsuccessful attempt as
  the 20-minute limit;
- at least 80% of participants passing U6 unaided within 5 minutes.

It exits 0 when every threshold holds and 1 when one does not. An unaided
success needs `success`, `assisted` of `no`, a passing check, and, for U1, a
correct explanation. Retest rows are reported separately and never replace
original results.

Report each round with [the results template](../usability/results.md). When a
misunderstanding recurs, change the documentation or syntax, record the
change, and repeat the affected tasks with fresh participants in a retest round.

## Checking the kit

`tests/usability.rs` keeps the kit honest in every test run. Each task's
reference solution, in `tests/usability`, must pass its check, and the untouched
starter files and a set of plausible wrong answers must fail it for the stated
reason. The U1 script must print the answer the check expects, and `score.py`
must compute every threshold, including its edges.
