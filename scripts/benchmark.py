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
import re
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

    def __init__(self, bridge: Path, variant: str | None = None):
        self.bridge = bridge
        self.variant = variant
        if variant:
            self.name = f"Trurl ({variant} engine)"

    def start(self, model, threads, cap):
        self.home = Path(tempfile.mkdtemp(prefix="bench-trurl-"))
        (self.home / "models").mkdir()
        os.link(model, self.home / "models" / model.name) if model.stat().st_dev == self.home.stat().st_dev else shutil.copy(model, self.home / "models")
        if self.variant:
            # The engine an older CPU would get, on this computer.
            (self.home / "settings.json").write_text(json.dumps({"max_variant": self.variant}))
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
        # The clock starts here, at the press of Send, as for the other apps: the
        # test's own "new chat" and event connection are not the learner's wait.
        self.t_send = time.time()
        self.call("send", {"text": prompt})
        finished = False
        try:
            for raw in r:
                line = raw.decode().strip()
                if not line.startswith("data:"):
                    continue
                ev = json.loads(line[5:])
                if ev["type"] == "turn_delta" and ev.get("answer"):
                    yield 1
                elif ev["type"] == "turn_finished":
                    finished = True
                    return
        finally:
            # The other apps stop at MAX_TOKENS by request; Trurl has no such
            # limit (learners see whole answers), so stop it the way a learner
            # would and wait until it is done, or the next question would wait
            # for this answer to be cancelled.
            if not finished:
                self.call("stop")
                for raw in r:
                    line = raw.decode().strip()
                    if line.startswith("data:") and json.loads(line[5:])["type"] == "turn_finished":
                        break
            r.close()

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
        # llamafile is an "actually portable executable": the shell starts it on any Linux
        cmd = ["sh", str(self.exe), "--server", "--gpu", "disable", "-m", str(model), "-c", "2048", "-t", str(threads), "--host", "127.0.0.1", "--port", str(self.port)]
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

def quartiles(xs: list[float]) -> list[float]:
    q = statistics.quantiles(xs, n=4, method="inclusive")
    return [q[0], q[2]]


def session(app: App, model: Path, threads: int, idle: bool) -> dict:
    """One start-to-stop session: load, warm up, answer every prompt once."""
    t0 = time.time()
    app.start(model, threads, None)
    while not app.ready():
        if app.proc.poll() is not None:
            raise RuntimeError(f"{app.name} exited during load")
        if time.time() - t0 > 600:
            raise RuntimeError(f"{app.name} did not load in 10 minutes")
        time.sleep(0.05)
    load = time.time() - t0
    app.configure()
    # Warm-up answer (excluded), then the measured ones.
    for _ in app.stream("Say OK."):
        pass
    out: dict = {"load": load, "first_words": [], "speeds": [], "peaks": []}
    for prompt in PROMPTS:
        sampler = PeakSampler(app.proc.pid)
        t_send = time.time()
        n, t_first, t_last = 0, None, None
        app.t_send = None
        for _ in app.stream(prompt):
            now = time.time()
            if t_first is None:
                t_first = now
            t_last = now
            n += 1
            if n >= MAX_TOKENS:
                break
        out["peaks"].append(sampler.done())
        if t_first is None:
            continue
        out["first_words"].append(t_first - (app.t_send or t_send))
        if n > 1:
            out["speeds"].append((n - 1) / (t_last - t_first))
    if idle:
        # Idle: model loaded, nothing asked.
        time.sleep(2)
        pids = tree(app.proc.pid)
        out["idle_ram"] = rss(pids)
        out["own_ram"] = rss([app.proc.pid])  # the app's own process, without its model engine
        c0 = cpu_ticks(pids)
        time.sleep(20)
        out["idle_cpu"] = (cpu_ticks(tree(app.proc.pid)) - c0) / TICK / 20 * 100
    app.stop()
    return out


def measure(apps: list[tuple[App, int]], model: Path, threads: int, rounds: int, log) -> list[dict]:
    """Apps take turns, round by round (A B C, A B C, ...), so a noisy minute on the
    machine hits every app instead of the one that happened to be running."""
    acc = {app.name: {"loads": [], "first_words": [], "speeds": [], "peaks": []} for app, _ in apps}
    for r in range(rounds):
        for app, _ in apps:
            s = session(app, model, threads, idle=(r == 0))
            a = acc[app.name]
            a["loads"].append(s["load"])
            for k in ("first_words", "speeds", "peaks"):
                a[k].extend(s[k])
            if r == 0:
                a["idle_ram"], a["idle_cpu"], a["own_ram"] = s["idle_ram"], s["idle_cpu"], s["own_ram"]
            log(f"round {r + 1}/{rounds} {app.name}: {statistics.median(s['speeds']):.1f} tokens/s, load {s['load']:.1f}s")
    results = []
    for app, disk in apps:
        a = acc[app.name]
        results.append({
            "app": app.name,
            "disk_mb": round(disk / 1e6),
            "load_s": round(statistics.median(a["loads"]), 1),
            "first_word_s": round(statistics.median(a["first_words"]), 2),
            "tokens_per_s": round(statistics.median(a["speeds"]), 1),
            # The middle half of the answers: one slow minute on the machine does not
            # make an app look faster or slower than it is.
            "tokens_per_s_range": [round(q, 1) for q in quartiles(a["speeds"])],
            "tokens_per_s_all": [round(x, 2) for x in a["speeds"]],
            "ram_idle_mb": round(a["idle_ram"] / 1e6),
            "ram_own_process_mb": round(a["own_ram"] / 1e6),
            "ram_peak_mb": round(max(a["peaks"]) / 1e6),
            "cpu_idle_pct": round(a["idle_cpu"], 2),
        })
    return results


def evict(path: Path) -> None:
    """Drop a file's pages from the page cache (no root needed for clean pages).

    Without this, a memory cap means little: pages already cached by an earlier
    run belong to another cgroup and are not charged to the capped app."""
    try:
        fd = os.open(path, os.O_RDONLY)
        try:
            os.fsync(fd)
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
    except OSError:
        pass


def evict_app_files(app: "App", model: Path) -> None:
    evict(model)
    # Ollama stores its own copy of the model as a blob.
    home = getattr(app, "home", None)
    if home:
        for f in Path(home).rglob("*"):
            if f.is_file() and f.stat().st_size > 50_000_000:
                evict(f)


def low_ram(app: App, model: Path, threads: int, cap: str) -> str:
    try:
        t0 = time.time()
        evict(model)
        app.start(model, threads, cap)
        evict_app_files(app, model)  # Ollama copied the model while starting: evict that copy too
        while not app.ready():
            if app.proc.poll() is not None:
                return "does not start"
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
        return f"answers in {max(1, round(time.time() - t1))} s" if n else "no answer"
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

    # Speed is noisy on a shared machine: every app whose range reaches the best
    # median is a tie, and all of them are bold.
    top = max(rows, key=lambda r: r["tokens_per_s"])

    def tied(r):
        lo, hi = r.get("tokens_per_s_range", [r["tokens_per_s"]] * 2)
        tlo, thi = top.get("tokens_per_s_range", [top["tokens_per_s"]] * 2)
        return hi >= tlo and lo <= thi

    def cell(r, k, f):
        v = f.format(r[k])
        if k == "tokens_per_s" and "tokens_per_s_range" in r:
            lo, hi = r["tokens_per_s_range"]
            v = f"{r[k]} tokens/s ({lo} to {hi})"
            return f"**{v}**" if tied(r) else v
        return f"**{v}**" if r[k] == best[k] else v

    head = "| | " + " | ".join(c[1] for c in cols) + " |\n|---|" + "---|" * len(cols) + "\n"
    table = head + "".join("| " + r["app"] + " | " + " | ".join(cell(r, k, f) for k, _, f in cols) + " |\n" for r in rows)
    caps = list(next(iter(data["low_ram"].values())).keys())
    low = "| Memory left for the app | " + " | ".join(caps) + " |\n|---|" + "---|" * len(caps) + "\n"
    def plain(v: str) -> str:
        # Older result files said it differently.
        m = re.match(r"answers \(\d+ tokens in (\d+)s\)", v)
        return f"answers in {max(1, int(m.group(1)))} s" if m else v.replace("fails to load", "does not start")

    low += "".join("| " + app + " | " + " | ".join(plain(v[c]) for c in caps) + " |\n" for app, v in data["low_ram"].items())
    setup = (f"{m.get('label') or m['cpu']}, {m['threads']} threads, {m['ram_gb']} GB RAM, no GPU. Model: `{data['model']}` for every app, "
             f"context 2,048, temperature 0, answers of up to {data['max_tokens']} tokens, median of {len(PROMPTS)} prompts "
             f"x {data.get('rounds', 2)} rounds (the apps take turns each round). Speed in brackets: the middle half of the answers. "
             f"Measured {data['date']} by `scripts/benchmark.py`.")
    more = data.get("trurl_more") or []
    tiers = ""
    if more:
        tiers = ("| Trurl | Model | Load time | First word | Speed | RAM, answering |\n|---|---|---|---|---|---|\n"
                 + "".join(f"| {r['engine']} engine | `{r['model']}` | {r['load_s']} s | {round(r['first_word_s'], 1)} s | "
                           f"{r['tokens_per_s']} tokens/s | {r['ram_peak_mb']:,} MB |\n" for r in more))
    readme = (table + "\n" + setup + "\n\n**With little free memory.** Each app started inside a memory limit, "
              "then asked one short question:\n\n" + low
              + ("\n**Older computers, and a bigger model.** The same computer, running the engine an older CPU would get "
                 "(avx: 2011 to 2012 processors; ssse3: 2006 to 2010; compat: anything older), and Gemma 3 4B. On ssse3, "
                 "models in the Q4_0 format answer two to four times faster than these Q4_K_M ones (docs/ARCHITECTURE.md).\n\n" + tiers if tiers else ""))
    full = (
        "# Benchmarks\n\nGenerated by `scripts/benchmark.py` from `docs/benchmarks.json`. Do not edit by hand: rerun the script.\n\n"
        "## Setup\n\n" + setup + "\n\n"
        "## Results\n\nBold is the best in each column. For speed, every app whose middle half overlaps the fastest one's is bold: "
        "the difference is within the noise of the machine.\n\n" + table + "\n"
        "- **App on disk:** the program without the model. For Trurl: the terminal app plus its four llama-server builds.\n"
        "- **Load time:** from starting the app to the first possible answer, model already in the disk cache. Median of the rounds.\n"
        "- **Speed:** words (tokens) per second as they appear while the answer streams. All three apps compute with the same "
        "library (ggml); Ollama 0.40 runs Gemma 3 with its own engine and streams it a little faster.\n"
        "- **Trurl** here is Trurl's core and its model engine, without a window: what the terminal app runs (the test drives it "
        "through the core's development bridge). The Windows window adds Microsoft's WebView2: the Windows smoke test in CI "
        "prints its memory.\n"
        "- **App's own process** (in the JSON, `ram_own_process_mb`): the app without its model engine.\n"
        "- **RAM:** proportional set size (PSS) of every process of the app, so shared pages count once. The model's 806 MB of weights are memory-mapped and counted here.\n"
        "- **CPU, idle:** with the model loaded and nothing asked, over 20 seconds. Anything above 0% drains a laptop battery for nothing.\n\n"
        "## When memory is short\n\nEach app started inside a memory limit (cgroup, no swap), then asked to answer one short question.\n\n" + low + "\n"
        + ("## Older computers, and a bigger model\n\nTrurl on the same computer with the engine an older CPU would get "
           "(avx: Sandy Bridge and Ivy Bridge, 2011 to 2012; ssse3: Core 2 and first Core i, 2006 to 2010; compat: any 64-bit CPU), "
           "and with Gemma 3 4B. The bundled Q4_K_M model is slow on ssse3 and compat; on ssse3, Q4_0 models answer two to four "
           "times faster (docs/ARCHITECTURE.md). An older CPU is also "
           "slower in itself, so expect less on a real one; the order of the engines stays the same.\n\n" + tiers + "\n" if tiers else "")
        + "## Reproduce\n\n```\npython3 scripts/benchmark.py --model ~/.cache/trurl/models/gemma-3-1b-it-Q4_K_M.gguf \\\n"
        "    --ollama /path/to/ollama --llamafile /path/to/llamafile --json docs/benchmarks.json \\\n"
        "    --tiers --big-model ~/.cache/trurl/models/gemma-3-4b-it-Q4_K_M.gguf\npython3 scripts/benchmark.py --render docs/benchmarks.json\n```\n"
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
    ap.add_argument("--machine-label", help="how to name the test machine in the docs (stored in the JSON)")
    ap.add_argument("--trurl", default=str(ROOT / "target/release/trurl-bridge"))
    ap.add_argument("--trurl-tui", default=str(ROOT / "target/release/trurl"))
    ap.add_argument("--ollama")
    ap.add_argument("--llamafile")
    ap.add_argument("--threads", type=int, default=os.cpu_count() or 2)
    ap.add_argument("--runs", type=int, default=5, help="rounds; each app answers every prompt once per round")
    ap.add_argument("--caps", default="1.5G,1G,750M,500M")
    ap.add_argument("--tiers", action="store_true", help="also run Trurl on its avx and compat engines (what older CPUs get)")
    ap.add_argument("--big-model", help="also run Trurl with this bigger model (Gemma 3 4B)")
    ap.add_argument("--extra-rounds", type=int, default=2, help="rounds for --tiers and --big-model")
    ap.add_argument("--only-low-ram", action="store_true", help="re-run only the memory-cap test, merging into --json")
    ap.add_argument("--json", default=str(ROOT / "target/bench.json"))
    a = ap.parse_args()
    if a.render:
        if a.machine_label:
            data = json.loads(Path(a.render).read_text())
            data["machine"]["label"] = a.machine_label
            Path(a.render).write_text(json.dumps(data, indent=2))
        write_docs(Path(a.render))
        return
    model = Path(a.model).expanduser().resolve()

    trurl_disk = dir_size(Path(a.trurl_tui)) + dir_size(ROOT / "vendor/llama/linux")
    apps: list[tuple[App, int]] = [(Trurl(Path(a.trurl)), trurl_disk)]
    if a.ollama:
        apps.append((Ollama(Path(a.ollama)), dir_size(Path(a.ollama))))
    if a.llamafile:
        apps.append((Llamafile(Path(a.llamafile)), dir_size(Path(a.llamafile))))

    lowram: dict = {}
    if a.only_low_ram:
        results = json.loads(Path(a.json).read_text())["results"]
    else:
        results = measure(apps, model, a.threads, a.runs, lambda m: print(m, flush=True))
        for r in results:
            print(json.dumps(r), flush=True)
    for app, _ in apps:
        print(f"== {app.name} with less memory", flush=True)
        lowram[app.name] = {}
        for cap in a.caps.split(","):
            lowram[app.name][cap] = low_ram(app, model, a.threads, cap)
            print(f"   {cap}: {lowram[app.name][cap]}", flush=True)

    extra = []
    if a.tiers:
        for v in ("avx", "ssse3", "compat"):
            print(f"== Trurl on the {v} engine", flush=True)
            r = measure([(Trurl(Path(a.trurl), v), trurl_disk)], model, a.threads, a.extra_rounds, lambda m: print(m, flush=True))[0]
            r["engine"], r["model"] = v, model.name
            extra.append(r)
            print(json.dumps(r), flush=True)
    if a.big_model:
        big = Path(a.big_model).expanduser().resolve()
        print(f"== Trurl with {big.name}", flush=True)
        r = measure([(Trurl(Path(a.trurl)), trurl_disk)], big, a.threads, a.extra_rounds, lambda m: print(m, flush=True))[0]
        r["engine"], r["model"] = "avx2", big.name
        extra.append(r)
        print(json.dumps(r), flush=True)

    meminfo = Path("/proc/meminfo").read_text().split("\n")[0].split()[1]
    out = {
        "machine": {"cpu": cpu_name(), "label": a.machine_label, "threads": a.threads, "ram_gb": round(int(meminfo) / 1024 / 1024, 1)},
        "model": model.name,
        "max_tokens": MAX_TOKENS,
        "rounds": a.runs,
        "results": results,
        "low_ram": lowram,
        "trurl_more": extra,
        "date": time.strftime("%Y-%m-%d"),
    }
    Path(a.json).parent.mkdir(parents=True, exist_ok=True)
    Path(a.json).write_text(json.dumps(out, indent=2))
    print("wrote", a.json)


if __name__ == "__main__":
    main()
