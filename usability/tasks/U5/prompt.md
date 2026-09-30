# U5: Add dataset cases

`totals.suite.botwork` is a suite with one named case, `total`, that checks a
line total. Change that case so that it runs once for each of these rows, where
the last number is the expected total:

| Quantity | Price | Expected total |
| --- | --- | --- |
| 3 | 4 | 12 |
| 2 | 7 | 99 |
| 0 | 7 | 0 |

1. Run the suite. The second row should fail, because its expectation is
   wrong; the other two should pass, and the run should report a failure.
2. Then run the suite again so that only the failed row runs. Write the
   arguments you gave `botwork` for this second run, exactly as typed, in
   `rerun.args`.

Do not fix the wrong expectation.
