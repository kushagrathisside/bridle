# Benchmark report

| Condition | Runs | Passed | Timeouts | Mean time (s) | Mean tool calls | Mean blocked | Mean context bytes before -> after |
|---|---|---|---|---|---|---|---|
| harness | 6 | 5 | 0 | 257 | 12.5 | 4.8 | 44640 -> 44640 |
| minimal | 6 | 3 | 0 | 171 | 4.7 | 0.8 | 6346 -> 6346 |

## Per task

| Task | Category | Condition | Passed | Time (s) | Tool calls | Blocked |
|---|---|---|---|---|---|---|
| analyse-latency | experiment_analysis | harness | yes | 382 | 27 | 17 |
| analyse-latency | experiment_analysis | minimal | no | 199 | 4 | 1 |
| comprehend-stock-path | repo_comprehension | harness | yes | 207 | 8 | 3 |
| comprehend-stock-path | repo_comprehension | minimal | no | 185 | 3 | 2 |
| debug-range-sum | debugging | harness | yes | 133 | 6 | 2 |
| debug-range-sum | debugging | minimal | no | 182 | 3 | 2 |
| implement-fizzbuzz | implementation | harness | no | 474 | 19 | 7 |
| implement-fizzbuzz | implementation | minimal | yes | 147 | 4 | 0 |
| refactor-rename | refactoring | harness | yes | 151 | 11 | 0 |
| refactor-rename | refactoring | minimal | yes | 211 | 10 | 0 |
| repair-wrong-test | test_repair | harness | yes | 195 | 4 | 0 |
| repair-wrong-test | test_repair | minimal | yes | 99 | 4 | 0 |

The sample is small: a few tasks, one run each. A difference in these numbers is a lead for the next run, not a measured effect. Tokens, VRAM and the first-pass rate are not measured by this run.
written: D:\src\harnessforkush\evals\bench\results\2026-10-28\minimal\report.md
