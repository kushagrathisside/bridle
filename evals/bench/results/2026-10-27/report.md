# Benchmark report

| Condition | Runs | Passed | Timeouts | Mean time (s) | Mean tool calls | Mean blocked | Mean context bytes before -> after |
|---|---|---|---|---|---|---|---|
| harness | 6 | 4 | 0 | 189 | 4.2 | 0.5 | 5293 -> 5293 |
| minimal | 6 | 6 | 0 | 185 | 5.5 | 0.5 | 8734 -> 8734 |

## Per task

| Task | Category | Condition | Passed | Time (s) | Tool calls | Blocked |
|---|---|---|---|---|---|---|
| analyse-latency | experiment_analysis | harness | yes | 194 | 3 | 1 |
| analyse-latency | experiment_analysis | minimal | yes | 195 | 2 | 0 |
| comprehend-stock-path | repo_comprehension | harness | no | 176 | 2 | 1 |
| comprehend-stock-path | repo_comprehension | minimal | yes | 200 | 9 | 2 |
| debug-range-sum | debugging | harness | no | 178 | 2 | 1 |
| debug-range-sum | debugging | minimal | yes | 138 | 4 | 1 |
| implement-fizzbuzz | implementation | harness | yes | 202 | 4 | 0 |
| implement-fizzbuzz | implementation | minimal | yes | 176 | 4 | 0 |
| refactor-rename | refactoring | harness | yes | 192 | 10 | 0 |
| refactor-rename | refactoring | minimal | yes | 209 | 10 | 0 |
| repair-wrong-test | test_repair | harness | yes | 192 | 4 | 0 |
| repair-wrong-test | test_repair | minimal | yes | 194 | 4 | 0 |

The sample is small: a few tasks, one run each. A difference in these numbers is a lead for the next run, not a measured effect. Tokens, VRAM and the first-pass rate are not measured by this run.
written: D:\bench-runs\m10-full-minimal\report.md
