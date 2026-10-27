#!/usr/bin/env python3
"""Subagent quality run (M4b-8): runs `agent subagent` over the cases in cases.json and scores each answer.

The agent runs on Windows, because the model servers listen on the Windows loopback and the workspace is a Windows folder.
So the script copies the fixtures into the workspace, writes one batch file that runs every case, runs it with cmd.exe,
and scores the output files. It makes no model call of its own.

Scoring reads only the answer body (`summary:`, the findings and `recommendation:`). The header and the diagnostic lines
contain words such as "not" and "no", so they are left out. A run passes when the body matches one of `must_any` and
none of `must_not`. A run that exits non-zero is counted as unavailable, not as a wrong answer.

usage: python3 evals/subagent/run.py --agent D:\\cargo-target\\harness\\debug\\agent.exe --workspace D:\\subagent-live
"""

import argparse
import datetime
import json
import re
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"
UNSAFE_IN_BAT = set('%&|<>^"')
HEALTH = "http://127.0.0.1:{port}/health"


def windows_to_wsl(path: str) -> Path:
    """`D:\\folder` becomes `/mnt/d/folder`."""
    drive, rest = path[0].lower(), path[2:].replace("\\", "/")
    return Path(f"/mnt/{drive}/{rest.lstrip('/')}")


def health(port: int) -> bool:
    """The Windows side answers the loopback check, so curl.exe is used from WSL."""
    result = subprocess.run(
        ["curl.exe", "--noproxy", "*", "-s", "--max-time", "5", HEALTH.format(port=port)],
        capture_output=True,
        text=True,
    )
    return result.returncode == 0 and result.stdout.strip() != ""


def answer_body(text: str) -> str:
    """The lines a reader acts on: the summary, the findings and the recommendation."""
    keep = [
        line
        for line in text.splitlines()
        if line.startswith(("summary:", "- ", "recommendation:"))
    ]
    return "\n".join(keep)


def score(text: str, case: dict) -> bool:
    body = answer_body(text)
    matched = any(re.search(p, body, re.IGNORECASE) for p in case["must_any"])
    forbidden = any(re.search(p, body, re.IGNORECASE) for p in case["must_not"])
    return matched and not forbidden


def batch(agent: str, workspace: str, cases: list, repeats: int, out: str) -> str:
    lines = ["@echo off", f"cd /d {workspace}"]
    for case in cases:
        if any(c in UNSAFE_IN_BAT for c in case["task"]):
            raise SystemExit(f"case {case['id']}: task has a character the batch file cannot quote")
        paths = " ".join(f"--path subagent-eval\\{p}" for p in case["paths"])
        for n in range(1, repeats + 1):
            target = f"{out}\\{case['id']}-{n}.txt"
            lines.append(
                f'"{agent}" subagent --role {case["role"]} --task "{case["task"]}" {paths} > "{target}" 2>&1'
            )
            lines.append(f'(echo EXIT=%ERRORLEVEL%)>> "{target}"')
    lines.append("echo SUBAGENT_EVAL_DONE")
    return "\r\n".join(lines) + "\r\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--agent", required=True, help="Windows path of agent.exe")
    parser.add_argument("--workspace", required=True, help="Windows path of an initialised workspace with [subagents] enabled")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--port", type=int, default=8002, help="port of the micro model (the subagent slot)")
    parser.add_argument("--out", help="Windows folder for the outputs (default: a timestamped folder under the workspace's drive root)")
    args = parser.parse_args()

    if not health(args.port):
        print(f"micro model not answering on port {args.port}; start it first (agent models serve micro --detach)")
        return 2

    cases = json.loads((HERE / "cases.json").read_text(encoding="utf-8"))["cases"]
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    out = args.out or f"{args.workspace[:3]}subagent-eval-runs\\run-{stamp}"
    out_wsl = windows_to_wsl(out)
    out_wsl.mkdir(parents=True, exist_ok=True)

    workspace_wsl = windows_to_wsl(args.workspace)
    fixtures_wsl = workspace_wsl / "subagent-eval"
    fixtures_wsl.mkdir(exist_ok=True)
    for source in FIXTURES.iterdir():
        (fixtures_wsl / source.name).write_bytes(source.read_bytes())

    bat_wsl = Path("/mnt/d/cargo-target") / f"subagent-eval-{stamp}.bat"
    bat_wsl.write_text(batch(args.agent, args.workspace, cases, args.repeats, out), encoding="ascii")
    bat_win = f"D:\\cargo-target\\subagent-eval-{stamp}.bat"

    run = subprocess.run(["cmd.exe", "/c", bat_win], capture_output=True, text=True)
    if "SUBAGENT_EVAL_DONE" not in run.stdout:
        print("the batch did not finish:", run.stdout[-800:], run.stderr[-400:])

    rows = []
    for case in cases:
        for n in range(1, args.repeats + 1):
            path = out_wsl / f"{case['id']}-{n}.txt"
            raw = path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""
            exit_line = [line for line in raw.splitlines() if line.startswith("EXIT=")]
            code = exit_line[-1][5:].strip() if exit_line else "missing"
            if code != "0" or not raw.startswith("[subagent "):
                status = "unavailable"
            else:
                status = "pass" if score(raw, case) else "fail"
            rows.append(
                {
                    "case": case["id"],
                    "run": n,
                    "status": status,
                    "dropped_references": "named no input" in raw,
                }
            )

    counts = {"pass": 0, "fail": 0, "unavailable": 0}
    for row in rows:
        counts[row["status"]] += 1
    answered = counts["pass"] + counts["fail"]
    print(f"runs {len(rows)}: pass {counts['pass']}, fail {counts['fail']}, unavailable {counts['unavailable']}")
    if answered:
        print(f"accuracy over answered runs: {counts['pass'] / answered:.2f}")
    for case in cases:
        mine = [r for r in rows if r["case"] == case["id"]]
        passed = sum(r["status"] == "pass" for r in mine)
        print(f"  {case['id']}: {passed}/{len(mine)}")
    (out_wsl / "results.json").write_text(json.dumps({"counts": counts, "rows": rows}, indent=2), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
