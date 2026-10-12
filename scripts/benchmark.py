#!/usr/bin/env python3
"""Benchmark Trurl against other ways to run the same model on the same computer.

Every app runs the SAME model file (gemma-3-1b-it-Q4_K_M.gguf), the same context
(2048), the same number of threads, temperature 0, and is asked the same
questions. Measured from outside, the way a learner's laptop would feel it:

  disk        size of the app on disk, without the model
  load        seconds from start until the first answer can begin
  first word  seconds from pressing Send to the first streamed word
  speed       words (tokens) per second while answering
  RAM idle    memory of the whole app (every process) with the model loaded
  RAM peak    the most memory the app used while answering
  CPU idle    CPU used while the app sits waiting (0% is what you want on a battery)
  low RAM     does it still answer when the computer has only 1.5 / 1 / 0.75 GB free?

Linux only (reads /proc; caps memory with systemd-run cgroups).

  python3 scripts/benchmark.py --model ~/.cache/trurl/models/gemma-3-1b-it-Q4_K_M.gguf \\
      --trurl target/release/trurl-bridge --ollama /opt/ollama --llamafile ~/llamafile-0.10.6 \\
      --out docs/BENCHMARKS.md --json target/bench.json
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import socket
import statistics
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROMPTS = [
    "Explain in three sentences why the sky is blue.",
    "Give three tips for studying for an exam.",
    "What is a language model? Answer in two sentences.",
]
MAX_TOKENS = 128
PAGE = os.sysconf("SC_PAGE_SIZE")
TICK = os.sysconf("SC_CLK_TCK")


# ----- /proc helpers ----------------------------------------------------------

def children_map() -> dict[int, list[int]]:
    kids: dict[int, list[int]] = {}
    for d in Path("/proc").iterdir():
        if not d.name.isdigit():
            continue
        try:
            stat = (d / "stat").read_text()
            ppid = int(stat[stat.rindex(")") + 2:].split()[1])
            kids.setdefault(ppid, []).append(int(d.name))
        except (OSError, ValueError):
            pass
    return kids


def tree(pid: int) -> list[int]:
    kids = children_map()
    out, stack = [], [pid]
    while stack:
        p = stack.pop()
        out.append(p)
        stack.extend(kids.get(p, []))
    return out


def rss(pids: list[int]) -> int:
    """Proportional set size (PSS) summed over the tree: shared pages counted once."""
    total = 0
    for p in pids:
        try:
            for line in Path(f"/proc/{p}/smaps_rollup").read_text().splitlines():
                if line.startswith("Pss:"):
                    total += int(line.split()[1]) * 1024
        except OSError:
            pass
    return total


def cpu_ticks(pids: list[int]) -> int:
    total = 0
    for p in pids:
        try:
            stat = Path(f"/proc/{p}/stat").read_text()
            f = stat[stat.rindex(")") + 2:].split()
            total += int(f[11]) + int(f[12])  # utime + stime
        except (OSError, ValueError, IndexError):
            pass
    return total


def dir_size(p: Path) -> int:
    if p.is_file():
        return p.stat().st_size
    return sum(f.stat().st_size for f in p.rglob("*") if f.is_file() and not f.is_symlink())


def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def http(method: str, url: str, body: dict | None = None, timeout: float = 600):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers={"content-type": "application/json"})
    return urllib.request.urlopen(req, timeout=timeout)


class PeakSampler:
    """Samples tree memory every 100 ms in the background."""

    def __init__(self, pid: int):
        self.pid, self.peak, self.stop = pid, 0, False
        import threading

        self.t = threading.Thread(target=self.run, daemon=True)
        self.t.start()

    def run(self):
        while not self.stop:
            self.peak = max(self.peak, rss(tree(self.pid)))
            time.sleep(0.1)

    def done(self) -> int:
        self.stop = True
        self.t.join()
        return self.peak


# ----- the apps -----------------------------------------------------------------

class App:
    name = "?"
    proc: subprocess.Popen | None = None

    def start(self, model: Path, threads: int, cap: str | None) -> None: ...
    def ready(self) -> bool: ...
    def stream(self, prompt: str):
        """Yield one item per streamed token."""
        raise NotImplementedError

    def wrap(self, cmd: list[str], cap: str | None) -> list[str]:
        if not cap:
            return cmd
        return ["systemd-run", "--user", "--scope", "-q", "-p", f"MemoryMax={cap}", "-p", "MemorySwapMax=0", *cmd]

    def stop(self) -> None:
        if self.proc and self.proc.poll() is None:
            for p in reversed(tree(self.proc.pid)):
                try:
                    os.kill(p, signal.SIGTERM)
                except OSError:
                    pass
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                for p in tree(self.proc.pid):
                    try:
                        os.kill(p, signal.SIGKILL)
                    except OSError:
                        pass


class Trurl(App):
    """The Trurl core (through the dev bridge, the same code the apps run) + its llama-server."""

    name = "Trurl"

    def __init__(self, bridge: Path):
        self.bridge = bridge

    def start(self, model, threads, cap):
        self.home = Path(tempfile.mkdtemp(prefix="bench-trurl-"))
        (self.home / "models").mkdir()
        os.link(model, self.home / "models" / model.name) if model.stat().st_dev == self.home.stat().st_dev else shutil.copy(model, self.home / "models")
        self.port = free_port()
        env = dict(os.environ, TRURL_HOME=str(self.home), TRURL_LLAMA_DIR=str(ROOT / "vendor/llama/linux"))
        self.proc = subprocess.Popen(self.wrap([str(self.bridge), "--port", str(self.port)], cap), env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.events = None

    def call(self, cmd, args=None):
        return json.load(http("POST", f"http://127.0.0.1:{self.port}/api/{cmd}", args or {}, timeout=30))

    def ready(self):
        try:
            return self.call("snapshot")["engine"]["state"] == "ready"
        except Exception:
            return False

    def configure(self):
        self.call("set_temperature", {"value": 0})

    def stream(self, prompt):
        self.call("new_chat")
        r = urllib.request.urlopen(f"http://127.0.0.1:{self.port}/events", timeout=600)
        self.call("send", {"text": prompt})
        for raw in r:
            line = raw.decode().strip()
            if not line.startswith("data:"):
                continue
            ev = json.loads(line[5:])
            if ev["type"] == "turn_delta" and ev.get("answer"):
                yield 1
            elif ev["type"] == "turn_finished":
                r.close()
                return

    def stop(self):
        super().stop()
        shutil.rmtree(self.home, ignore_errors=True)


class OpenAIServer(App):
    """Anything with an OpenAI-style streaming endpoint (llamafile)."""

    def stream(self, prompt):
        body = {"messages": [{"role": "user", "content": prompt}], "stream": True, "temperature": 0, "max_tokens": MAX_TOKENS}
        r = http("POST", f"http://127.0.0.1:{self.port}/v1/chat/completions", body)
        for raw in r:
            line = raw.decode().strip()
            if line == "data: [DONE]":
                return
            if line.startswith("data:"):
                ch = json.loads(line[5:]).get("choices") or [{}]
                if (ch[0].get("delta") or {}).get("content"):
                    yield 1

    def ready(self):
        try:
            return http("GET", f"http://127.0.0.1:{self.port}/health", timeout=5).status == 200
        except Exception:
            return False


class Llamafile(OpenAIServer):
    name = "llamafile"

    def __init__(self, exe: Path):
        self.exe = exe

    def start(self, model, threads, cap):
        self.port = free_port()
        cmd = [str(self.exe), "--server", "--nobrowser", "-m", str(model), "-c", "2048", "-t", str(threads), "--host", "127.0.0.1", "--port", str(self.port)]
        self.proc = subprocess.Popen(self.wrap(cmd, cap), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def configure(self):
        pass


class Ollama(App):
    name = "Ollama"

    def __init__(self, root: Path):
        self.root = root

    def start(self, model, threads, cap):
        self.home = Path(tempfile.mkdtemp(prefix="bench-ollama-"))
        self.port = free_port()
        self.threads = threads
        env = dict(os.environ, OLLAMA_HOST=f"127.0.0.1:{self.port}", OLLAMA_MODELS=str(self.home / "models"), HOME=str(self.home),
                   OLLAMA_NUM_PARALLEL="1", OLLAMA_MAX_LOADED_MODELS="1", OLLAMA_KEEP_ALIVE="-1", OLLAMA_CONTEXT_LENGTH="2048", CUDA_VISIBLE_DEVICES="")
        self.env = env
        self.proc = subprocess.Popen(self.wrap([str(self.root / "bin" / "ollama"), "serve"], cap), env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # Import the very same GGUF file.
        for _ in range(100):
            try:
                http("GET", f"http://127.0.0.1:{self.port}/api/version", timeout=2)
                break
            except Exception:
                time.sleep(0.2)
        (self.home / "Modelfile").write_text(f"FROM {model}\n")
        subprocess.run([str(self.root / "bin" / "ollama"), "create", "bench", "-f", str(self.home / "Modelfile")], env=env, check=True, capture_output=True)
        self.loaded = False

    def ready(self):
        if self.loaded:
            return True
        try:
            body = {"model": "bench", "messages": [{"role": "user", "content": "hi"}], "stream": False,
                    "options": {"num_ctx": 2048, "num_thread": self.threads, "temperature": 0, "num_predict": 1}}
            http("POST", f"http://127.0.0.1:{self.port}/api/chat", body, timeout=600).read()
            self.loaded = True
        except Exception:
            return False
        return True

    def configure(self):
        pass

    def stream(self, prompt):
        body = {"model": "bench", "messages": [{"role": "user", "content": prompt}], "stream": True,
                "options": {"num_ctx": 2048, "num_thread": self.threads, "temperature": 0, "num_predict": MAX_TOKENS}}
        r = http("POST", f"http://127.0.0.1:{self.port}/api/chat", body)
        for raw in r:
            ev = json.loads(raw)
            if ev.get("message", {}).get("content"):
                yield 1
            if ev.get("done"):
                return

    def stop(self):
        super().stop()
        shutil.rmtree(self.home, ignore_errors=True)


# ----- measuring -----------------------------------------------------------------

def measure(app: App, model: Path, threads: int, disk: int, runs: int) -> dict:
    t0 = time.time()
    app.start(model, threads, None)
    while not app.ready():
        if app.proc.poll() is not None:
            raise RuntimeError(f"{app.name} exited during load")
        if time.time() - t0 > 600:
            raise RuntimeError(f"{app.name} did not load in 10 minutes")
        time.sleep(0.1)
    app.configure()
    # Warm-up answer (excluded), then the measured ones.
    for _ in app.stream("Say OK."):
        pass
    load = None
    first_words, speeds, peaks = [], [], []
    for i in range(runs):
        for prompt in PROMPTS:
            sampler = PeakSampler(app.proc.pid)
            t_send = time.time()
            n, t_first, t_last = 0, None, None
            for _ in app.stream(prompt):
                now = time.time()
                if t_first is None:
                    t_first = now
                t_last = now
                n += 1
                if n >= MAX_TOKENS:
                    break
            peaks.append(sampler.done())
            if t_first is None:
                continue
            first_words.append(t_first - t_send)
            if n > 1:
                speeds.append((n - 1) / (t_last - t_first))
    # Idle: model loaded, nothing asked.
    time.sleep(2)
    pids = tree(app.proc.pid)
    idle_ram = rss(pids)
    c0 = cpu_ticks(pids)
    time.sleep(20)
    idle_cpu = (cpu_ticks(tree(app.proc.pid)) - c0) / TICK / 20 * 100
    app.stop()
    # Load time measured separately, from a cold start of the app (page cache warm).
    t0 = time.time()
    app.start(model, threads, None)
    while not app.ready():
        time.sleep(0.05)
    load = time.time() - t0
    app.stop()
    return {
        "app": app.name,
        "disk_mb": round(disk / 1e6),
        "load_s": round(load, 1),
        "first_word_s": round(statistics.median(first_words), 2),
        "tokens_per_s": round(statistics.median(speeds), 1),
        "ram_idle_mb": round(idle_ram / 1e6),
        "ram_peak_mb": round(max(peaks) / 1e6),
        "cpu_idle_pct": round(idle_cpu, 2),
    }


def low_ram(app: App, model: Path, threads: int, cap: str) -> str:
    try:
        t0 = time.time()
        app.start(model, threads, cap)
        while not app.ready():
            if app.proc.poll() is not None:
                return "fails to load"
            if time.time() - t0 > 300:
                return "too slow to load (>5 min)"
            time.sleep(0.2)
        app.configure()
        t1 = time.time()
        n = 0
        for _ in app.stream("Say hello in one short sentence."):
            n += 1
            if time.time() - t1 > 180:
                return "too slow (>3 min)"
        return f"answers ({n} tokens in {time.time() - t1:.0f}s)" if n else "no answer"
    except Exception as e:
        return f"fails ({type(e).__name__})"
    finally:
        app.stop()


def render(data: dict) -> tuple[str, str]:
    """(README table, full BENCHMARKS.md) from the measured JSON. Never edit the numbers by hand."""
    m = data["machine"]
    rows = data["results"]
    best = {
        "disk_mb": min(r["disk_mb"] for r in rows),
        "load_s": min(r["load_s"] for r in rows),
        "first_word_s": min(r["first_word_s"] for r in rows),
        "tokens_per_s": max(r["tokens_per_s"] for r in rows),
        "ram_idle_mb": min(r["ram_idle_mb"] for r in rows),
        "ram_peak_mb": min(r["ram_peak_mb"] for r in rows),
        "cpu_idle_pct": min(r["cpu_idle_pct"] for r in rows),
    }
    cols = [
        ("disk_mb", "App on disk", "{:,} MB"),
        ("load_s", "Load time", "{} s"),
        ("first_word_s", "First word", "{} s"),
        ("tokens_per_s", "Speed", "{} tokens/s"),
        ("ram_idle_mb", "RAM, idle", "{:,} MB"),
        ("ram_peak_mb", "RAM, answering", "{:,} MB"),
        ("cpu_idle_pct", "CPU, idle", "{}%"),
    ]

    def cell(r, k, f):
        v = f.format(r[k])
        return f"**{v}**" if r[k] == best[k] else v

    head = "| | " + " | ".join(c[1] for c in cols) + " |\n|---|" + "---|" * len(cols) + "\n"
    table = head + "".join("| " + r["app"] + " | " + " | ".join(cell(r, k, f) for k, _, f in cols) + " |\n" for r in rows)
    caps = list(next(iter(data["low_ram"].values())).keys())
    low = "| Memory left for the app | " + " | ".join(caps) + " |\n|---|" + "---|" * len(caps) + "\n"
    low += "".join("| " + app + " | " + " | ".join(v[c] for c in caps) + " |\n" for app, v in data["low_ram"].items())
    setup = (f"{m['cpu']}, {m['threads']} threads, {m['ram_gb']} GB RAM, no GPU. Model: `{data['model']}` for every app, "
             f"context 2,048, temperature 0, {data['max_tokens']}-token answers, median of {len(PROMPTS)} prompts x runs. "
             f"Measured {data['date']} by `scripts/benchmark.py`.")
    readme = table + "\n" + setup + "\n"
    full = (
        "# Benchmarks\n\nGenerated by `scripts/benchmark.py` from `docs/benchmarks.json`. Do not edit by hand: rerun the script.\n\n"
        "## Setup\n\n" + setup + "\n\n"
        "## Results\n\nBold is the best in each column.\n\n" + table + "\n"
        "- **App on disk:** the program without the model. For Trurl: the terminal app plus its three llama-server builds.\n"
        "- **Load time:** from starting the app to the first possible answer, model already in the disk cache.\n"
        "- **RAM:** proportional set size (PSS) of every process of the app, so shared pages count once. The model's 806 MB of weights are memory-mapped and counted here.\n"
        "- **CPU, idle:** with the model loaded and nothing asked, over 20 seconds. Anything above 0% drains a laptop battery for nothing.\n\n"
        "## When memory is short\n\nEach app started inside a memory limit (cgroup, no swap), then asked to answer one short question.\n\n" + low + "\n"
        "## Reproduce\n\n```\npython3 scripts/benchmark.py --model ~/.cache/trurl/models/gemma-3-1b-it-Q4_K_M.gguf \\\n"
        "    --ollama /path/to/ollama --llamafile /path/to/llamafile --json docs/benchmarks.json\npython3 scripts/benchmark.py --render docs/benchmarks.json\n```\n"
    )
    return readme, full


def write_docs(json_path: Path) -> None:
    data = json.loads(json_path.read_text())
    readme_table, full = render(data)
    (ROOT / "docs" / "BENCHMARKS.md").write_text(full)
    readme = (ROOT / "README.md").read_text()
    start, end = "<!-- benchmarks:start -->", "<!-- benchmarks:end -->"
    if start in readme:
        a = readme.index(start) + len(start)
        b = readme.index(end)
        readme = readme[:a] + "\n" + readme_table + readme[b:]
        (ROOT / "README.md").write_text(readme)
    print("wrote docs/BENCHMARKS.md and the README table")


def cpu_name() -> str:
    for line in Path("/proc/cpuinfo").read_text().splitlines():
        if line.startswith("model name"):
            return line.split(":", 1)[1].strip()
    return "?"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model")
    ap.add_argument("--render", help="only rebuild docs/BENCHMARKS.md and the README table from this JSON")
    ap.add_argument("--trurl", default=str(ROOT / "target/release/trurl-bridge"))
    ap.add_argument("--trurl-tui", default=str(ROOT / "target/release/trurl"))
    ap.add_argument("--ollama")
    ap.add_argument("--llamafile")
    ap.add_argument("--threads", type=int, default=os.cpu_count() or 2)
    ap.add_argument("--runs", type=int, default=2)
    ap.add_argument("--caps", default="1.5G,1G,750M,500M")
    ap.add_argument("--json", default=str(ROOT / "target/bench.json"))
    a = ap.parse_args()
    if a.render:
        write_docs(Path(a.render))
        return
    model = Path(a.model).expanduser().resolve()

    trurl_disk = dir_size(Path(a.trurl_tui)) + dir_size(ROOT / "vendor/llama/linux")
    apps: list[tuple[App, int]] = [(Trurl(Path(a.trurl)), trurl_disk)]
    if a.ollama:
        apps.append((Ollama(Path(a.ollama)), dir_size(Path(a.ollama))))
    if a.llamafile:
        apps.append((Llamafile(Path(a.llamafile)), dir_size(Path(a.llamafile))))

    results, lowram = [], {}
    for app, disk in apps:
        print(f"== {app.name}", flush=True)
        r = measure(app, model, a.threads, disk, a.runs)
        print(json.dumps(r), flush=True)
        results.append(r)
        lowram[app.name] = {}
        for cap in a.caps.split(","):
            lowram[app.name][cap] = low_ram(app, model, a.threads, cap)
            print(f"   {cap}: {lowram[app.name][cap]}", flush=True)

    meminfo = Path("/proc/meminfo").read_text().split("\n")[0].split()[1]
    out = {
        "machine": {"cpu": cpu_name(), "threads": a.threads, "ram_gb": round(int(meminfo) / 1024 / 1024, 1)},
        "model": model.name,
        "max_tokens": MAX_TOKENS,
        "results": results,
        "low_ram": lowram,
        "date": time.strftime("%Y-%m-%d"),
    }
    Path(a.json).parent.mkdir(parents=True, exist_ok=True)
    Path(a.json).write_text(json.dumps(out, indent=2))
    print("wrote", a.json)


if __name__ == "__main__":
    main()
