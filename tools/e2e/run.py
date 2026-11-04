r"""End-to-end suite for the Bridle window, native Windows, no WSL.

    python tools\e2e\run.py --exe target\debug\bridle.exe [--skip-model] [--keep]

It makes a throwaway project (a copy of examples/toy-python-repo with a git history), launches the program's window with
WebView2's debug port open, runs the checks of suite.json through the bounded client in cdp.py, closes the window the way
the X button does, and then checks that no `bridle.exe` or `llama-server.exe` is left running. With --skip-model the checks
that need a model are skipped (no GPU use). The exit code is 0 only when every check passed and nothing was left behind.
"""
import argparse, json, os, shutil, struct, subprocess, sys, tempfile, time, urllib.request, zlib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
PORT = 9222


def running(image):
    out = subprocess.run(["tasklist", "/FI", f"IMAGENAME eq {image}", "/NH"], capture_output=True, text=True, timeout=30).stdout
    return sum(1 for line in out.splitlines() if line.strip().lower().startswith(image.lower()))


def make_image(path):
    w, h = 320, 200
    rows = []
    for y in range(h):
        row = bytearray([0])
        for x in range(w):
            if 30 <= x < 130 and 50 <= y < 150: px = (220, 30, 30)
            elif (x - 230) ** 2 + (y - 100) ** 2 < 50 ** 2: px = (30, 60, 220)
            else: px = (255, 255, 255)
            row += bytes(px)
        rows.append(bytes(row))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
    with open(path, "wb") as f: f.write(png)


def make_project(path, files):
    os.makedirs(path)
    for name, content in files.items():
        with open(os.path.join(path, name), "w", encoding="utf-8", newline="\n") as f: f.write(content)
    git = ["git", "-c", "user.name=e2e", "-c", "user.email=e2e@example.com"]
    subprocess.run(["git", "init", "-q"], cwd=path, check=True)
    subprocess.run(["git", "add", "-A"], cwd=path, check=True)
    subprocess.run(git + ["commit", "-q", "-m", "init"], cwd=path, check=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--skip-model", action="store_true")
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("--port", type=int, default=PORT)
    args = ap.parse_args()
    exe = os.path.abspath(args.exe)
    if not os.path.isfile(exe): sys.exit(f"not found: {exe}")
    if running("bridle.exe") or running("llama-server.exe"):
        sys.exit("bridle.exe or llama-server.exe is already running; close it first so the result means something")

    base = tempfile.mkdtemp(prefix="bridle-e2e-")
    project, other = os.path.join(base, "project"), os.path.join(base, "other")
    toy = os.path.join(ROOT, "examples", "toy-python-repo")
    make_project(project, {n: open(os.path.join(toy, n), encoding="utf-8").read() for n in os.listdir(toy) if os.path.isfile(os.path.join(toy, n))})
    make_project(other, {"hello.py": 'print("other project")\n'})
    image = os.path.join(base, "shapes.png")
    make_image(image)

    def esc(p): return p.replace("\\", "\\\\")
    steps = json.load(open(os.path.join(HERE, "suite.json"), encoding="utf-8"))
    chosen = [s for s in steps if not (args.skip_model and s.get("model"))]
    for s in chosen:
        s["eval"] = s["eval"].replace("{{IMAGE}}", esc(image)).replace("{{OTHER}}", esc(other)).replace("{{PROJECT}}", esc(project))
        s.pop("model", None)
    steps_file = os.path.join(base, "steps.json")
    json.dump(chosen, open(steps_file, "w"))

    env = {k: v for k, v in os.environ.items() if k.upper() not in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY")}
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={args.port}"
    print(f"project {project}\nlaunching {exe}", flush=True)
    proc = subprocess.Popen([exe, "--gui", project], cwd=project, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ok = False
    try:
        deadline = time.time() + 60
        while time.time() < deadline:
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{args.port}/json/list", timeout=3).read(); break
            except Exception: time.sleep(1)
        else:
            sys.exit("the window did not open its debug port within 60 s")
        time.sleep(6)
        result = subprocess.run([sys.executable, os.path.join(HERE, "cdp.py"), steps_file, "--port", str(args.port), "--limit", "900", "--read-timeout", "200"], timeout=1000)
        ok = result.returncode == 0
        servers = running("llama-server.exe")
        print(f"model servers running before close: {servers}", flush=True)
        if servers > 1: ok = False; print("FAIL: more than one model server")
    finally:
        subprocess.run(["powershell", "-NoProfile", "-Command", "$p = Get-Process bridle -ErrorAction SilentlyContinue; if ($p) { [void]$p.CloseMainWindow() }"], timeout=30)
        for _ in range(20):
            time.sleep(1)
            if not running("bridle.exe") and not running("llama-server.exe"): break
        left = (running("bridle.exe"), running("llama-server.exe"))
        print(f"after close: bridle.exe={left[0]} llama-server.exe={left[1]}", flush=True)
        if left != (0, 0): ok = False; print("FAIL: something was left running")
        if proc.poll() is None:
            pass
        if not args.keep: shutil.rmtree(base, ignore_errors=True)
    print("PASS" if ok else "FAIL", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
