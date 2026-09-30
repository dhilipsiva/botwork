# Usability pilot

Per [roadmap decision D2](decisions.md#d2-usability-sessions), two AI sessions
attempted the six usability tasks before any human session, to find confusing
documentation and syntax early. **This pilot is formative. It never counts
toward the usability thresholds, and it is no substitute for the human
sessions**, which the [usability study](usability-study.md) describes.

## Setup

On 2026-09-30, each pilot got a folder holding:

- the `botwork` build of revision `265b3c8`, SHA-256 `76d3f41a…74b699`;
- a copy of the published documentation: `README.md`, `docs/` without the
  study's own pages, and `examples/`;
- a workspace made by `usability/prepare.py`.

Each pilot started a fresh session, with no access to how Botwork or the kit was
written. Its instructions were to work only in its folder, read only that
documentation, never run `order.botwork` for U1, and keep a log of the start
and end time, documentation read, attempts, and confusions for every task.
The facilitator then ran `usability/check.py` on each workspace.

| Pilot | Persona | Model | Task order |
| --- | --- | --- | --- |
| P01 | Experienced automation practitioner | Claude Opus 5.5 | U1 U2 U3 U4 U5 U6 |
| P02 | Automation newcomer | Claude Haiku 4.5 | U1 U2 U4 U5 U3 U6 |

## Results

| Task | P01 check | P01 time | P02 check |
| --- | --- | ---: | --- |
| U1: Read | pass | 39 s | fail: `answer.txt` held the explanation too |
| U2: First run | pass | 5 s | pass |
| U3: Reuse | pass | 9 s | pass |
| U4: Compose | pass | 9 s | pass |
| U5: Dataset | pass | 25 s | pass |
| U6: Repair | pass | 14 s | pass |

P02 predicted U1's seven lines correctly but wrote its explanation into
`answer.txt` as well, so the check, which requires exactly the predicted lines,
failed it. Only P01's explanation meets the rubric: P02 said the last line is
99 because `quantity` was set to 99 at the start, without saying that the
statement's `quantity` parameter is separate from the caller's, so a
facilitator would score its explanation as incorrect. The times say little: P01 did
nearly all its reading during U1, and P02's logged timestamps are not real
clock readings, so its times are not reported.

## Findings

P01 reported the confusions below. P02 reported none, which, given its log, is
weak evidence that there were none.

| Finding | Where | Action |
| --- | --- | --- |
| U1's prompt did not say that `answer.txt` holds only the predicted lines | Kit | The prompt now says so, and asks for the explanation out loud |
| After U4 forbade editing the helper, it was unclear whether U6 allowed it | Kit | U6's prompt now says either file may be edited |
| A parameter shadowing a caller's variable of the same name is explained in one sentence of the language guide, and not in getting started | Docs | Getting started now says a statement's parameters and variables are its own |
| A statement must be defined before a line that calls it runs; this is easy to miss | Docs | Added to getting started |
| How pipes pair up inside `@{ ... }`, and that the left side of an assignment takes pipes too | Docs | Added to getting started |
| No example of a module-qualified call inside `@{ ... }` | Docs | Getting started now shows `@{ pricing::Total of \|3\| at \|4\| }` |
| `--failures` leaves a `failed.json.lock` that getting started does not mention | Docs | Getting started now explains it |
| Rerunning failures needs `--failures` on the first run, and the failure summary does not say how to rerun | Product | Done: a suite's [failure recap](console-report.md#failure-recap) ends with the rerun command |
| BW2001's help begins "Define `discont`…", which points toward adding a variable rather than fixing a misspelling, and there is no "did you mean `discount`" | Product | Done: undefined variables [suggest a near name](diagnostics.md#near-name-suggestions) |
| Call frames show a statement's normalized name, `pricing::linetotalof\|param\|at\|param\|less\|param\|`, rather than the name as written | Product | Done: frames show [statements as written](diagnostics.md#statements-as-written) |
| An imported file's path is shown in full, the entry script's relative | Product | Open: consistent paths |

The product findings are items under the usability sessions in
[TODO.md](../TODO.md).

## Limitations

- An AI persona is not a person. Both pilots come from one model family, so
  they can share blind spots, and they read documentation faster and more
  completely than people do.
- The rules were instructions, not enforcement: nothing technically stopped a
  pilot from reading outside its folder. P01 reported reading only its
  documentation copy. P02's log is unreliable: its timestamps are not real, so
  its reported reading cannot be taken on trust either.
- The prompts and getting started changed after the pilot, as above. The human
  sessions use the revised versions.
