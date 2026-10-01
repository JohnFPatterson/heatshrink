#!/usr/bin/python3
"""C-to-Rust parity gate: a Cursor `stop` and `subagentStop` hook.

When an agent finishes in a workspace that looks like a C-to-Rust port
(Cargo.toml at the root plus C sources, or an explicit .cursor/parity.json),
this hook runs the original C and the Rust port over every fixture itself and
compares stdout and exit status byte for byte. Any divergence that is not a
PARITY_EXCEPTIONS.md row naming that fixture, with a test that exists and
passes, sends the agent back to work through `followup_message`.

Per-port contract: .cursor/parity.json (see parity.template.json next to this
script). Commands are argv arrays and never go through a shell.

Modules (optional) split the compare so a multi-agent port is checked piece by
piece:
  "modules": {"parse": {"driver_args": ["--sections", "parse"], "ready": false,
                        "fixtures": ["tests/parse/*"]}}
`driver_args` is appended to both c_cmd and rust_cmd; `fixtures` (optional)
replaces the top-level fixture globs for that module. The first time a module
is seen with "ready": true its definition is pinned; setting it back to false,
removing it, or changing its driver_args or fixtures is then a gate problem.
  - `--event subagentStop`: runs every ready module and sends the subagent back
    if one diverges. With no module ready, only the pins are checked.
  - `stop` (the default event): fails while any module is not ready (ready
    modules are still run and reported). Once all are ready it runs the full
    compare, exactly as without modules.
  - `--module NAME` (repeatable): runs the named modules by hand, ready or not.
  - Without `modules`, both events run the full compare.
`report_dir` (optional, relative to the repo root; default the root) is where
reports go: the full compare and `stop` write parity-report.{md,json}; module
runs (subagentStop, --module) write parity-report.modules.{md,json}.

State lives outside the repo in ./state/<sha256 of repo path>.json:
  - oracle_pins:  sha256 of every file matched by `oracle_sources`
  - fixture_pins: sha256 of every fixture ever seen, top-level and module globs
                  (fixtures may be added, never removed or edited)
  - module_pins:  definition of every module ever seen ready (only with modules)
  - last:         tree hash and verdict of the last full-compare run (reused
                  when nothing changed and the report on disk is from that run)
  - last_modules: the same for module runs, keyed by mode and module set
To re-baseline after an intentional change to the oracle, fixtures, or a ready
module, the user deletes that state file.

Reproduce a run by hand:
  printf '%s' '{"status":"completed","loop_count":0,"workspace_roots":["/path/to/repo"]}' \
    | ./.cursor/hooks/c-rust-parity/parity_gate.py --force
Add `--event subagentStop` for the ready-modules run, or `--module NAME`.

Known limits:
  - Cursor's `loop_limit` (5 in hooks.json) caps forced follow-ups. After that
    the agent may stop; parity-report.md still says FAIL.
  - Cursor fails open if the hook times out, so the script keeps its own
    budget (BUDGET_S) below the hooks.json timeout and reports an overrun as a
    failure.
  - An agent with shell access could edit or delete the state file. That
    resets the pins; it is deliberate and visible, not impossible.
  - Pins are taken on the first gate run. Edits to the C made before that run
    are not detected.
  - The gate only checks that the Rust command is not the same executable as
    the C command. A Rust driver that shells out to the C oracle would pass.
  - Unexpected hook input (non-JSON stdin) is logged and allowed through,
    because the hook cannot tell which workspace it belongs to.
  - Without `workspace_roots` in the payload the gate falls back to the
    payload's `cwd`, then CURSOR_PROJECT_DIR, then its own working directory.
    Cursor starts hooks in ~/.cursor, so that last fallback finds no port and
    allows the stop.
  - subagentStop checks modules, not subagents. The gate cannot tell which
    subagent owns which module: every subagent stop runs every ready module,
    and a failure goes to whichever subagent stopped.
  - A subagent that never sets its module to ready is not checked when it
    stops. It is caught only at the final stop, which fails while any module
    is not ready.
  - Module runs do not flag stale exception rows, because a row may be needed
    only by the full compare. The full compare at the final stop flags them.
"""
from __future__ import annotations

import fnmatch
import glob
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import Any

HOOK_VERSION = "2"
HOOK_PATH = Path(__file__).resolve()
HOOK_DIR = HOOK_PATH.parent
STATE_DIR = HOOK_DIR / "state"
TEMPLATE_PATH = HOOK_DIR / "parity.template.json"

CONFIG_REL = ".cursor/parity.json"
RULE_REL = ".cursor/rules/c-to-rust-port.mdc"
REPORT_MD = "parity-report.md"
REPORT_JSON = "parity-report.json"
REPORT_MODULES_MD = "parity-report.modules.md"
REPORT_MODULES_JSON = "parity-report.modules.json"
EVENTS = ("stop", "subagentStop")
MODULE_NAME_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")
MODULE_KEYS = {"driver_args", "ready", "fixtures", "$comment"}

# Must stay below the `timeout` in hooks.json (900 s): Cursor fails open on timeout.
BUDGET_S = 840.0
MAX_LISTED = 5
HEX_CONTEXT = 16
MAX_FOLLOWUP_CHARS = 8000
WALK_LIMIT = 50000
SKIP_DIRS = {".git", "target", "node_modules", ".venv", "venv", "__pycache__"}
BASE_HASH_EXCLUDE = ["target/**", REPORT_MD, REPORT_JSON]
PLACEHOLDERS = {"", "-", "--", "\u2014", "\u2013", "n/a", "none"}

CONFIG_DEFAULTS: dict[str, Any] = {
    "enabled": True,
    "reason": "",
    "build": None,
    "input_mode": "arg",
    "oracle_sources": ["*.c", "*.h"],
    "exceptions_file": "PARITY_EXCEPTIONS.md",
    "exceptions_rs": ["*-core/tests/exceptions.rs"],
    "exceptions_test": ["cargo", "test", "--test", "exceptions"],
    "per_run_timeout_s": 10,
    "build_timeout_s": 600,
    "compare_stderr": False,
    "hash_exclude": [],
    "report_dir": "",
    "modules": None,
}
CONFIG_KEYS = set(CONFIG_DEFAULTS) | {"c_cmd", "rust_cmd", "fixtures", "$comment"}

FINAL_INSTRUCTION = (
    "Fix the Rust port, or log a PARITY_EXCEPTIONS.md row with a test. "
    "Do not edit the C oracle."
)


def log(msg: str) -> None:
    print(f"parity-gate: {msg}", file=sys.stderr)


class ConfigError(Exception):
    pass


class Deadline:
    def __init__(self, seconds: float) -> None:
        self.end = time.monotonic() + seconds

    def remaining(self) -> float:
        return self.end - time.monotonic()

    def expired(self) -> bool:
        return self.remaining() <= 0


# ---------------------------------------------------------------- utilities


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def skipped_path(rel: str) -> bool:
    return any(part in SKIP_DIRS for part in rel.split("/")[:-1])


def matches_any(rel: str, patterns: list[str]) -> bool:
    return any(fnmatch.fnmatchcase(rel, p) for p in patterns)


def list_files(root: Path) -> list[str]:
    """Repo files as sorted relative POSIX paths: git tracked + untracked-not-ignored, else a bounded walk."""
    if (root / ".git").exists():
        try:
            r = subprocess.run(
                ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                capture_output=True,
                timeout=60,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            r = None
        if r is not None and r.returncode == 0:
            rels = {p for p in r.stdout.decode("utf-8", "surrogateescape").split("\0") if p}
            return sorted(p for p in rels if not skipped_path(p) and (root / p).is_file())
    out: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        for name in filenames:
            full = Path(dirpath) / name
            if full.is_file():
                out.append(full.relative_to(root).as_posix())
            if len(out) >= WALK_LIMIT:
                log(f"{root}: file walk stopped at {WALK_LIMIT} files")
                return sorted(out)
    return sorted(out)


def glob_files(root: Path, patterns: list[str]) -> list[str]:
    root_resolved = root.resolve()
    found = set()
    for pattern in patterns:
        for p in root.glob(pattern):
            if not p.is_file():
                continue
            try:
                p.resolve().relative_to(root_resolved)
            except ValueError:
                continue
            rel = p.relative_to(root).as_posix()
            if not skipped_path(rel):
                found.add(rel)
    return sorted(found)


def argv_str(argv: list[str] | None) -> str:
    if not argv:
        return "(none)"
    return " ".join(a if a and not re.search(r"[\s'\"\\$`]", a) else json.dumps(a) for a in argv)


def tail(text: str, limit: int = 2000) -> str:
    text = text.strip()
    return text if len(text) <= limit else "..." + text[-limit:]


def md_cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ")


# ---------------------------------------------------------------- config


def _argv_field(cfg: dict[str, Any], key: str) -> list[str] | None:
    v = cfg.get(key)
    if v is None:
        return None
    if not (isinstance(v, list) and v and all(isinstance(x, str) and x for x in v)):
        raise ConfigError(f"`{key}` must be a non-empty array of strings (argv; no shell)")
    return v


def _globs_field(cfg: dict[str, Any], key: str, allow_empty: bool, label: str | None = None) -> list[str]:
    label = label or key
    v = cfg.get(key)
    if not (isinstance(v, list) and all(isinstance(x, str) and x for x in v)):
        raise ConfigError(f"`{label}` must be an array of glob strings")
    if not v and not allow_empty:
        raise ConfigError(f"`{label}` must not be empty")
    for pattern in v:
        if pattern.startswith("/") or ".." in pattern.split("/"):
            raise ConfigError(f"`{label}` pattern {pattern!r} must be relative to the repo root without `..`")
    return v


def _report_dir_field(cfg: dict[str, Any]) -> str:
    v = cfg.get("report_dir")
    if not isinstance(v, str):
        raise ConfigError("`report_dir` must be a string (a directory relative to the repo root)")
    v = v.strip()
    while v.startswith("./"):
        v = v[2:]
    v = v.rstrip("/")
    if v == ".":
        v = ""
    if v.startswith("/") or "\\" in v or ".." in v.split("/"):
        raise ConfigError(f"`report_dir` {v!r} must be relative to the repo root without `..`")
    return v


def _modules_field(cfg: dict[str, Any]) -> dict[str, dict[str, Any]] | None:
    v = cfg.get("modules")
    if v is None:
        return None
    if not isinstance(v, dict) or not [k for k in v if k != "$comment"]:
        raise ConfigError("`modules` must be an object naming at least one module")
    out: dict[str, dict[str, Any]] = {}
    for name, m in v.items():
        if name == "$comment":
            continue
        if not MODULE_NAME_RE.fullmatch(name):
            raise ConfigError(f"module name {name!r} must match {MODULE_NAME_RE.pattern}")
        if not isinstance(m, dict):
            raise ConfigError(f"`modules.{name}` must be an object")
        unknown = sorted(set(m) - MODULE_KEYS)
        if unknown:
            raise ConfigError(f"unknown key(s) in `modules.{name}`: {', '.join(unknown)}")
        args = m.get("driver_args")
        if not (isinstance(args, list) and all(isinstance(x, str) and x for x in args)):
            raise ConfigError(
                f"`modules.{name}.driver_args` must be an array of strings (argv appended to c_cmd and rust_cmd)"
            )
        if not isinstance(m.get("ready"), bool):
            raise ConfigError(f"`modules.{name}.ready` must be true or false")
        fixtures = _globs_field(m, "fixtures", False, f"modules.{name}.fixtures") if "fixtures" in m else None
        out[name] = {"driver_args": args, "ready": m["ready"], "fixtures": fixtures}
    return out


def load_config(root: Path) -> dict[str, Any]:
    path = root / CONFIG_REL
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError) as e:
        raise ConfigError(f"cannot read {CONFIG_REL}: {e}") from e
    except json.JSONDecodeError as e:
        raise ConfigError(f"{CONFIG_REL} is not valid JSON: {e}") from e
    if not isinstance(raw, dict):
        raise ConfigError(f"{CONFIG_REL} must be a JSON object")
    unknown = sorted(set(raw) - CONFIG_KEYS)
    if unknown:
        raise ConfigError(f"unknown key(s) in {CONFIG_REL}: {', '.join(unknown)}")
    cfg = dict(CONFIG_DEFAULTS)
    cfg.update(raw)

    if not isinstance(cfg["enabled"], bool):
        raise ConfigError("`enabled` must be true or false")
    if not isinstance(cfg["reason"], str):
        raise ConfigError("`reason` must be a string")
    cfg["report_dir"] = _report_dir_field(cfg)
    if not cfg["enabled"]:
        return cfg

    for key in ("c_cmd", "rust_cmd", "fixtures"):
        if key not in raw:
            raise ConfigError(f"`{key}` is required")
    cfg["c_cmd"] = _argv_field(cfg, "c_cmd")
    cfg["rust_cmd"] = _argv_field(cfg, "rust_cmd")
    cfg["build"] = _argv_field(cfg, "build")
    cfg["exceptions_test"] = _argv_field(cfg, "exceptions_test")
    if cfg["exceptions_test"] is None:
        raise ConfigError("`exceptions_test` must not be null")
    if cfg["input_mode"] not in ("arg", "stdin"):
        raise ConfigError('`input_mode` must be "arg" or "stdin"')
    if cfg["input_mode"] == "arg" and not any("{input}" in a for a in cfg["c_cmd"] + cfg["rust_cmd"]):
        raise ConfigError('`input_mode` is "arg" but neither command contains `{input}`')
    cfg["fixtures"] = _globs_field(cfg, "fixtures", allow_empty=False)
    cfg["oracle_sources"] = _globs_field(cfg, "oracle_sources", allow_empty=False)
    cfg["exceptions_rs"] = _globs_field(cfg, "exceptions_rs", allow_empty=False)
    cfg["hash_exclude"] = _globs_field(cfg, "hash_exclude", allow_empty=True)
    if not isinstance(cfg["exceptions_file"], str) or not cfg["exceptions_file"]:
        raise ConfigError("`exceptions_file` must be a non-empty string")
    for key in ("per_run_timeout_s", "build_timeout_s"):
        v = cfg[key]
        if isinstance(v, bool) or not isinstance(v, (int, float)) or v <= 0:
            raise ConfigError(f"`{key}` must be a positive number")
    if not isinstance(cfg["compare_stderr"], bool):
        raise ConfigError("`compare_stderr` must be true or false")
    cfg["modules"] = _modules_field(cfg)
    return cfg


def report_base(root: Path, cfg: dict[str, Any]) -> Path:
    base = root / cfg["report_dir"] if cfg["report_dir"] else root
    try:
        base.resolve().relative_to(root.resolve())
    except ValueError as e:
        raise ConfigError(f"`report_dir` {cfg['report_dir']!r} resolves outside the repo") from e
    if base.exists() and not base.is_dir():
        raise ConfigError(f"`report_dir` {cfg['report_dir']!r} exists and is not a directory")
    return base


def report_excludes(cfg: dict[str, Any]) -> list[str]:
    prefix = glob.escape(cfg["report_dir"]) + "/" if cfg["report_dir"] else ""
    return [prefix + n for n in (REPORT_MD, REPORT_JSON, REPORT_MODULES_MD, REPORT_MODULES_JSON)]


def module_definition(m: dict[str, Any]) -> dict[str, Any]:
    return {"driver_args": m["driver_args"], "fixtures": m["fixtures"]}


def module_definition_sha(m: dict[str, Any]) -> str:
    return sha256_bytes(json.dumps(module_definition(m), sort_keys=True, separators=(",", ":")).encode("utf-8"))


def check_module_pins(
    modules: dict[str, dict[str, Any]] | None, state: dict[str, Any]
) -> tuple[dict[str, Any], list[str], list[str]]:
    """Enforce the one-way ready rule. Return (updated module_pins, problems, notes)."""
    old = state.get("module_pins")
    pins: dict[str, Any] = dict(old) if isinstance(old, dict) else {}
    problems: list[str] = []
    notes: list[str] = []
    for name in sorted(pins):
        pin = pins[name]
        pinned_sha = pin.get("definition_sha256") if isinstance(pin, dict) else None
        if modules is None or name not in modules:
            problems.append(
                f"module `{name}` was marked ready and has been removed from `{CONFIG_REL}`; "
                "a ready module may not be removed"
            )
        elif not modules[name]["ready"]:
            problems.append(f"module `{name}` was marked ready and has been set back to not ready; ready is one-way")
        elif module_definition_sha(modules[name]) != pinned_sha:
            pinned_def = pin.get("definition") if isinstance(pin, dict) else None
            problems.append(
                f"module `{name}` changed its definition after it was marked ready: pinned "
                f"{json.dumps(pinned_def)}, now {json.dumps(module_definition(modules[name]))}"
            )
    added = []
    for name, m in (modules or {}).items():
        if m["ready"] and name not in pins:
            pins[name] = {"definition_sha256": module_definition_sha(m), "definition": module_definition(m)}
            added.append(name)
    if added:
        notes.append("newly pinned ready modules: " + ", ".join(f"`{n}`" for n in added))
    if pins:
        notes.append(f"{len(pins)} ready module{'s' if len(pins) != 1 else ''} pinned")
    return pins, problems, notes


# ---------------------------------------------------------------- state


def state_path(root: Path) -> Path:
    return STATE_DIR / (hashlib.sha256(str(root).encode("utf-8")).hexdigest()[:32] + ".json")


def load_state(root: Path) -> tuple[dict[str, Any], str | None]:
    path = state_path(root)
    if not path.exists():
        return {}, None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
        return {}, f"state file {path} is unreadable ({e}); the user must inspect and delete it"
    if not isinstance(data, dict) or data.get("repo") != str(root):
        return {}, f"state file {path} does not belong to this repo; the user must inspect and delete it"
    return data, None


def save_state(root: Path, state: dict[str, Any]) -> None:
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    state["repo"] = str(root)
    path = state_path(root)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(tmp, path)


# ---------------------------------------------------------------- running


class RunResult:
    def __init__(self) -> None:
        self.returncode: int | None = None
        self.timed_out = False
        self.start_error: str | None = None
        self.stdout = b""
        self.stderr = b""

    def status(self) -> str:
        if self.start_error is not None:
            return "not started"
        if self.timed_out:
            return "timeout"
        if self.returncode is not None and self.returncode < 0:
            return f"signal {-self.returncode}"
        return f"exit {self.returncode}"


def run(argv: list[str], cwd: Path, timeout: float, stdin_bytes: bytes | None = None) -> RunResult:
    res = RunResult()
    if timeout <= 0:
        res.timed_out = True
        return res
    try:
        proc = subprocess.Popen(
            argv,
            cwd=str(cwd),
            stdin=subprocess.PIPE if stdin_bytes is not None else subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
    except OSError as e:
        res.start_error = f"cannot start {argv_str(argv)}: {e}"
        return res
    try:
        res.stdout, res.stderr = proc.communicate(stdin_bytes, timeout=timeout)
    except subprocess.TimeoutExpired:
        res.timed_out = True
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            proc.kill()
        res.stdout, res.stderr = proc.communicate()
    res.returncode = proc.returncode
    return res


def substitute(argv: list[str], rel: str) -> list[str]:
    return [a.replace("{input}", rel) for a in argv]


def first_diff(a: bytes, b: bytes) -> int | None:
    if a == b:
        return None
    n = min(len(a), len(b))
    step = 4096
    for start in range(0, n, step):
        if a[start:start + step] != b[start:start + step]:
            for i in range(start, min(start + step, n)):
                if a[i] != b[i]:
                    return i
    return n


def context(data: bytes, offset: int) -> str:
    lo = max(0, offset - HEX_CONTEXT)
    chunk = data[lo:offset + HEX_CONTEXT]
    return f"@{lo}: {chunk.hex(' ') or '(empty)'}  {chunk!r}"


def describe_divergence(rel: str, c: RunResult, r: RunResult, compare_stderr: bool, module: str | None = None) -> str:
    lines = [f"- `{rel}`" + (f" (module `{module}`)" if module else "")]
    if c.status() != r.status():
        lines.append(f"  status differs: C {c.status()}, Rust {r.status()}")
    for label, a, b, enabled in (("stdout", c.stdout, r.stdout, True), ("stderr", c.stderr, r.stderr, compare_stderr)):
        if not enabled:
            continue
        off = first_diff(a, b)
        if off is None:
            continue
        lines.append(f"  {label} differs at byte {off} (C {len(a)} bytes, Rust {len(b)} bytes)")
        lines.append(f"    C    {context(a, off)}")
        lines.append(f"    Rust {context(b, off)}")
    return "\n".join(lines)


def identical(c: RunResult, r: RunResult, compare_stderr: bool) -> bool:
    if c.timed_out or r.timed_out or c.start_error or r.start_error:
        return False
    if c.returncode != r.returncode or c.stdout != r.stdout:
        return False
    return not compare_stderr or c.stderr == r.stderr


def same_executable(root: Path, a: str, b: str) -> bool:
    def resolve(cmd: str) -> Path | None:
        if "/" in cmd:
            p = (root / cmd)
            return p.resolve() if p.exists() else None
        for d in os.environ.get("PATH", "").split(os.pathsep):
            p = Path(d) / cmd
            if p.is_file():
                return p.resolve()
        return None

    ra, rb = resolve(a), resolve(b)
    return ra is not None and ra == rb


# ---------------------------------------------------------------- exceptions


def _clean(cell: str) -> str:
    return cell.strip().replace("**", "").strip("`").strip()


def _fixture_path(cell: str) -> str:
    path = cell.strip().strip("`").strip()
    return path.removeprefix("./")


def _split_row(line: str) -> list[str]:
    body = line.strip().removeprefix("|").removesuffix("|")
    return [c.strip() for c in re.split(r"(?<!\\)\|", body)]


def parse_exceptions(text: str) -> tuple[list[dict[str, Any]], list[str]]:
    rows: list[dict[str, Any]] = []
    problems: list[str] = []
    lines = text.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        if not line.strip().startswith("|"):
            i += 1
            continue
        header = [_clean(c).lower() for c in _split_row(line)]
        if not {"id", "test", "fixture"} <= set(header):
            i += 1
            continue
        idx = {name: header.index(name) for name in ("id", "test", "fixture")}
        i += 1
        while i < len(lines) and lines[i].strip().startswith("|"):
            raw_cells = _split_row(lines[i])
            lineno = i + 1
            i += 1
            if all(re.fullmatch(r":?-{3,}:?", c.strip()) for c in raw_cells if c.strip()):
                continue
            cells = [_clean(c) for c in raw_cells]
            if len(cells) != len(header):
                problems.append(f"PARITY_EXCEPTIONS line {lineno}: expected {len(header)} cells, found {len(cells)}")
                continue
            row_id = cells[idx["id"]]
            if row_id.lower() in PLACEHOLDERS:
                continue
            test = cells[idx["test"]]
            test = test.split("::")[-1].replace("()", "").strip("` ")
            fixtures = [_fixture_path(f) for f in re.split(r",|;|<br\s*/?>", cells[idx["fixture"]])]
            fixtures = [f for f in fixtures if f.lower() not in PLACEHOLDERS]
            if test.lower() in PLACEHOLDERS:
                problems.append(f"exception {row_id} has no test name")
                test = ""
            rows.append({"id": row_id, "test": test, "fixtures": fixtures, "line": lineno})
    return rows, problems


def check_exception_tests(
    root: Path, cfg: dict[str, Any], rows: list[dict[str, Any]], deadline: Deadline, method: dict[str, Any]
) -> tuple[set[str], list[str]]:
    """Return (ids of rows whose test exists and passed, problems)."""
    problems: list[str] = []
    if not rows:
        method["exceptions_test"] = "not run (no exception rows)"
        return set(), problems
    rs_files = glob_files(root, cfg["exceptions_rs"])
    sources = ""
    for rel in rs_files:
        try:
            sources += (root / rel).read_text(encoding="utf-8", errors="replace") + "\n"
        except OSError as e:
            problems.append(f"cannot read {rel}: {e}")
    if not rs_files:
        problems.append(f"exception rows exist but no file matches exceptions_rs {cfg['exceptions_rs']}")

    candidates = []
    for row in rows:
        if not row["test"]:
            continue
        if not re.search(rf"\bfn\s+{re.escape(row['test'])}\s*\(", sources):
            problems.append(f"exception {row['id']}: test `{row['test']}` not found in {', '.join(rs_files) or 'exceptions_rs'}")
        else:
            candidates.append(row)

    res = run(cfg["exceptions_test"], root, min(cfg["build_timeout_s"], deadline.remaining()))
    method["exceptions_test"] = f"{argv_str(cfg['exceptions_test'])} ({res.status()})"
    output = res.stdout.decode("utf-8", "replace") + "\n" + res.stderr.decode("utf-8", "replace")
    if res.start_error:
        problems.append(res.start_error)
        return set(), problems
    if res.timed_out or res.returncode != 0:
        problems.append(f"exception tests failed ({res.status()}): {argv_str(cfg['exceptions_test'])}\n{tail(output)}")
        return set(), problems
    passed: set[str] = set()
    for row in candidates:
        if re.search(rf"^test (?:\S+::)?{re.escape(row['test'])} \.\.\. ok$", output, re.MULTILINE):
            passed.add(row["id"])
        else:
            problems.append(
                f"exception {row['id']}: test `{row['test']}` did not run and pass under "
                f"{argv_str(cfg['exceptions_test'])} (ignored, filtered, or missing?)"
            )
    return passed, problems


# ---------------------------------------------------------------- reports


def report_paths(base: Path, modules_run: bool) -> tuple[Path, Path]:
    if modules_run:
        return base / REPORT_MODULES_MD, base / REPORT_MODULES_JSON
    return base / REPORT_MD, base / REPORT_JSON


def report_is_from(json_path: Path, tree_hash: str, run_key: str) -> bool:
    try:
        data = json.loads(json_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return False
    return isinstance(data, dict) and data.get("tree_hash") == tree_hash and data.get("run_key") == run_key


def write_reports(root: Path, report: dict[str, Any], md_path: Path, json_path: Path) -> None:
    md_path.parent.mkdir(parents=True, exist_ok=True)
    json_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    m = report["method"]
    out: list[str] = ["# Parity report", ""]
    out.append(f"Result: {report['result']}. {report['summary']}")
    out += ["", "## Method", ""]
    out.append(f"- Workspace: `{root}`")
    out.append(f"- Tree hash: `{report['tree_hash']}`")
    for label, key in (
        ("Build", "build"),
        ("C oracle", "c_cmd"),
        ("Rust port", "rust_cmd"),
        ("Input", "input"),
        ("Compared", "compared"),
        ("Fixture globs", "fixtures"),
        ("Modules run", "modules"),
        ("Modules not run", "modules_not_run"),
        ("Exceptions file", "exceptions_file"),
        ("Exception tests", "exceptions_test"),
    ):
        if key in m:
            out.append(f"- {label}: {m[key]}")
    out += ["", "Reproduce:", "", "```sh", report["reproduce"], "```", ""]

    out += ["## Per-fixture results", ""]
    with_module = any("module" in f for f in report["fixtures"])
    if report["fixtures"]:
        if with_module:
            out.append("| Module | Fixture | Result | C status | Rust status | C stdout sha256 | Rust stdout sha256 |")
            out.append("|---|---|---|---|---|---|---|")
        else:
            out.append("| Fixture | Result | C status | Rust status | C stdout sha256 | Rust stdout sha256 |")
            out.append("|---|---|---|---|---|---|")
        for f in report["fixtures"]:
            module_cell = f"`{md_cell(f['module'])}` | " if with_module else ""
            out.append(
                f"| {module_cell}`{md_cell(f['fixture'])}` | {f['result']} | {f['c_status']} | {f['rust_status']} "
                f"| `{f['c_stdout_sha256'][:12]}` | `{f['rust_stdout_sha256'][:12]}` |"
            )
    else:
        out.append("None.")
    out += ["", "## Divergences", ""]
    out += report["divergence_details"] or ["None."]
    out += ["", "## Exceptions used", ""]
    if report["exceptions_used"]:
        out.append("| ID | Fixture | Test |")
        out.append("|---|---|---|")
        for e in report["exceptions_used"]:
            out.append(f"| {md_cell(e['id'])} | `{md_cell(e['fixture'])}` | `{md_cell(e['test'])}` |")
    else:
        out.append("None.")
    out += ["", "## Gate problems", ""]
    out += [f"- {p}" for p in report["problems"]] or ["None."]
    out += ["", "## Pins", ""]
    out += [f"- {p}" for p in report["pins"]] or ["None."]
    out += ["", "## Not covered", ""]
    out += [f"- {n}" for n in report["not_covered"]]
    out.append("")
    md_path.write_text("\n".join(out), encoding="utf-8")


def reproduce_cmd(root: Path, flags: str = "--force") -> str:
    payload = json.dumps({"status": "completed", "loop_count": 0, "workspace_roots": [str(root)]})
    return f"printf '%s' '{payload}' | '{HOOK_PATH}' {flags}"


# ---------------------------------------------------------------- gate


def missing_config_message(root: Path) -> str:
    return (
        f"Parity gate: `{root}` looks like a C-to-Rust port (Cargo.toml plus C sources) but has no "
        f"`{CONFIG_REL}`. Create it from `{TEMPLATE_PATH}`: set `c_cmd` (an oracle driver that runs the "
        "original, unmodified C), `rust_cmd` (a driver that runs the Rust port the same way), and `fixtures` "
        "(globs covering every existing test input and golden file). The gate then compares both byte for "
        "byte on every stop. If this repo is not a C-to-Rust port, create the file with "
        '{"enabled": false, "reason": "<why>"} instead. Do not edit the C oracle.'
    )


def gate_root(root: Path, force: bool, event: str = "stop", module_sel: list[str] | None = None) -> str | None:
    """Return a followup message when the agent must keep working, else None."""
    has_config = (root / CONFIG_REL).is_file()
    if not has_config:
        if not (root / "Cargo.toml").is_file():
            return None
        files = list_files(root)
        if not any(f.endswith(".c") for f in files):
            return None
        log(f"{root}: port detected without {CONFIG_REL}")
        return missing_config_message(root)

    try:
        cfg = load_config(root)
        rbase = report_base(root, cfg)
    except ConfigError as e:
        return f"Parity gate: `{root}/{CONFIG_REL}` is invalid: {e}. Template: `{TEMPLATE_PATH}`. Do not edit the C oracle."

    if not cfg["enabled"]:
        markers = [m for m in (cfg["exceptions_file"], RULE_REL) if (root / m).exists()]
        if markers:
            return (
                f"Parity gate: `{CONFIG_REL}` opts out, but this repo has C-to-Rust port markers "
                f"({', '.join(markers)}), so the opt-out is ignored. Fill in `c_cmd`, `rust_cmd`, and `fixtures` "
                f"from `{TEMPLATE_PATH}`. Do not edit the C oracle."
            )
        reason = cfg["reason"].strip() or "(no reason given)"
        log(f"{root}: opted out: {reason}")
        skipped: dict[str, Any] = {
            "result": "SKIPPED",
            "summary": f"Opted out in {CONFIG_REL}: {reason}",
            "tree_hash": "",
            "method": {},
            "reproduce": reproduce_cmd(root),
            "counts": {},
            "fixtures": [],
            "divergence_details": [],
            "exceptions_used": [],
            "problems": [],
            "pins": [],
            "not_covered": ["Everything: the parity gate is disabled for this repo."],
        }
        write_reports(root, skipped, *report_paths(rbase, False))
        return None

    modules: dict[str, dict[str, Any]] | None = cfg["modules"]
    selected = list(dict.fromkeys(module_sel or []))
    not_ready: list[str] = []
    if selected:
        if modules is None:
            return f"Parity gate: `--module` was given but `{root}/{CONFIG_REL}` has no `modules`."
        unknown = [n for n in selected if n not in modules]
        if unknown:
            return (
                f"Parity gate: unknown module(s) {', '.join(f'`{n}`' for n in unknown)}; `{CONFIG_REL}` defines "
                + ", ".join(f"`{n}`" for n in modules) + "."
            )
        kind, run_names = "modules", selected
        flags = " ".join(f"--module {n}" for n in selected) + " --force"
    elif modules is None:
        kind, run_names, flags = "full", [], "--force"
    elif event == "subagentStop":
        kind, run_names = "modules", [n for n, m in modules.items() if m["ready"]]
        flags = "--event subagentStop --force"
    else:
        not_ready = [n for n, m in modules.items() if not m["ready"]]
        run_names = [n for n, m in modules.items() if m["ready"]]
        kind, flags = ("partial" if not_ready else "full"), "--force"
    cache_key = "full" if kind == "full" else f"{kind}:{','.join(run_names)}"
    not_run = [n for n in modules if n not in run_names] if modules is not None and kind != "full" else []
    md_path, json_path = report_paths(rbase, kind == "modules")

    deadline = Deadline(BUDGET_S)
    excludes = BASE_HASH_EXCLUDE + report_excludes(cfg) + cfg["hash_exclude"]
    files = list_files(root)
    th = hashlib.sha256()
    th.update(f"v{HOOK_VERSION}:{sha256_file(HOOK_PATH)}\n".encode())
    file_hashes: dict[str, str] = {}
    for rel in files:
        if matches_any(rel, excludes):
            continue
        digest = sha256_file(root / rel)
        file_hashes[rel] = digest
        th.update(f"{rel}\0{digest}\n".encode("utf-8", "surrogateescape"))
    tree_hash = th.hexdigest()

    state, state_problem = load_state(root)
    if state_problem:
        return f"Parity gate: {state_problem}. Stop and tell the user. {FINAL_INSTRUCTION}"
    last_modules = state.get("last_modules")
    last_modules = dict(last_modules) if isinstance(last_modules, dict) else {}
    last = (state.get("last") if kind == "full" else last_modules.get(cache_key)) or {}
    if not isinstance(last, dict):
        last = {}
    # Pins only grow, so reverting to an older tree can violate them while its cached verdict still matches.
    module_pins, module_problems, module_notes = check_module_pins(modules, state)
    # Modes share report files, so after a revert the report on disk may come from another run; re-run then.
    if (
        not force
        and not module_problems
        and last.get("tree_hash") == tree_hash
        and report_is_from(json_path, tree_hash, cache_key)
    ):
        if last.get("verdict") in ("PASS", "SKIPPED"):
            log(f"{root}: unchanged since last {last['verdict']}" + ("" if kind == "full" else f" ({cache_key})"))
            return None
        if last.get("followup"):
            log(f"{root}: unchanged since last FAIL")
            return "Nothing changed since the last parity run, which failed.\n\n" + str(last["followup"])

    problems: list[str] = []
    pins_notes: list[str] = []

    oracle_now = {
        rel: file_hashes.get(rel) or sha256_file(root / rel)
        for rel in files
        if matches_any(rel, cfg["oracle_sources"])
    }
    if not oracle_now:
        problems.append(f"`oracle_sources` {cfg['oracle_sources']} matched no files")
    oracle_pins: dict[str, str] = state.get("oracle_pins") or {}
    if not oracle_pins:
        oracle_pins = dict(oracle_now)
        pins_notes.append(f"{len(oracle_pins)} oracle files pinned on this first run")
    else:
        changed = sorted(r for r in oracle_pins if r in oracle_now and oracle_now[r] != oracle_pins[r])
        missing = sorted(r for r in oracle_pins if r not in oracle_now)
        added = sorted(r for r in oracle_now if r not in oracle_pins)
        if changed:
            problems.append("C oracle files changed since they were pinned: " + ", ".join(f"`{r}`" for r in changed)
                            + ". Restore them (e.g. `git checkout -- <file>`); the C sources are the spec")
        if missing:
            problems.append("C oracle files removed since they were pinned: " + ", ".join(f"`{r}`" for r in missing))
        if added:
            for r in added:
                oracle_pins[r] = oracle_now[r]
            pins_notes.append("newly pinned oracle files: " + ", ".join(f"`{r}`" for r in added))
        pins_notes.append(f"{len(oracle_pins)} oracle files pinned")

    fixtures = glob_files(root, cfg["fixtures"])
    if not fixtures:
        problems.append(f"`fixtures` {cfg['fixtures']} matched no files; zero fixtures is never a pass")
    module_fixtures = {
        n: glob_files(root, m["fixtures"]) if m["fixtures"] else fixtures for n, m in (modules or {}).items()
    }
    all_fixtures = sorted(set(fixtures).union(*module_fixtures.values()))
    fixture_now = {rel: file_hashes.get(rel) or sha256_file(root / rel) for rel in all_fixtures}
    fixture_pins: dict[str, str] = state.get("fixture_pins") or {}
    f_changed = sorted(r for r in fixture_pins if r in fixture_now and fixture_now[r] != fixture_pins[r])
    f_missing = sorted(r for r in fixture_pins if r not in fixture_now)
    if f_changed:
        problems.append("fixtures edited since they were pinned: " + ", ".join(f"`{r}`" for r in f_changed[:20]))
    if f_missing:
        problems.append(
            f"fixture count dropped: {len(f_missing)} previously checked fixture(s) are gone or no longer matched: "
            + ", ".join(f"`{r}`" for r in f_missing[:20])
        )
    f_added = [r for r in fixture_now if r not in fixture_pins]
    for r in f_added:
        fixture_pins[r] = fixture_now[r]
    if f_added and state.get("fixture_pins"):
        pins_notes.append(f"{len(f_added)} new fixtures pinned")
    pins_notes.append(f"{len(fixture_pins)} fixtures pinned")

    problems += module_problems
    pins_notes += module_notes

    if kind == "full":
        units: list[tuple[str | None, list[str], list[str]]] = [(None, [], fixtures)]
    else:
        mods = modules or {}
        units = [(n, mods[n]["driver_args"], module_fixtures[n]) for n in run_names]
        for n in run_names:
            if mods[n]["fixtures"] and not module_fixtures[n]:
                problems.append(f"module `{n}` fixtures {mods[n]['fixtures']} matched no files")
    if kind == "partial":
        problems.insert(0, (
            "module(s) not ready: " + ", ".join(f"`{n}`" for n in not_ready)
            + f". The final stop requires every module in `{CONFIG_REL}` to be ready (each owner sets "
            "`modules.<name>.ready` to true once its sections match C); the full compare runs only then"
        ))
    planned = sum(len(u[2]) for u in units)

    if same_executable(root, cfg["c_cmd"][0], cfg["rust_cmd"][0]):
        problems.append("`c_cmd` and `rust_cmd` run the same executable")

    method: dict[str, Any] = {
        "build": "(none)",
        "c_cmd": f"`{argv_str(cfg['c_cmd'])}`",
        "rust_cmd": f"`{argv_str(cfg['rust_cmd'])}`",
        "input": "fixture path substituted for `{input}`" if cfg["input_mode"] == "arg" else "fixture bytes on stdin",
        "compared": "stdout bytes and exit status" + (" and stderr bytes" if cfg["compare_stderr"] else " (stderr not compared)"),
        "fixtures": ", ".join(f"`{g}`" for g in cfg["fixtures"]) + f" ({len(fixtures)} files)",
        "exceptions_file": f"`{cfg['exceptions_file']}`",
    }
    if modules is not None and kind != "full":
        labels = []
        for name in run_names:
            args, own_globs, fx = modules[name]["driver_args"], modules[name]["fixtures"], module_fixtures[name]
            globs = f"fixtures {', '.join(f'`{g}`' for g in own_globs)}, " if own_globs else ""
            labels.append(
                f"`{name}` (`{argv_str(args) if args else '(no extra args)'}`, {globs}"
                f"{len(fx)} fixture{'s' if len(fx) != 1 else ''})"
            )
        method["modules"] = ", ".join(labels) or "None (no module is ready)."
        method["modules_not_run"] = ", ".join(
            f"`{n}` ({'ready' if modules[n]['ready'] else 'not ready'})" for n in not_run
        ) or "None."

    build_ok = True
    if cfg["build"] and not units:
        method["build"] = f"`{argv_str(cfg['build'])}` (not run: no module to compare)"
    elif cfg["build"]:
        b = run(cfg["build"], root, min(cfg["build_timeout_s"], deadline.remaining()))
        method["build"] = f"`{argv_str(cfg['build'])}` ({b.status()})"
        if b.start_error or b.timed_out or b.returncode != 0:
            build_ok = False
            out = b.stdout.decode("utf-8", "replace") + "\n" + b.stderr.decode("utf-8", "replace")
            problems.append(f"build failed ({b.start_error or b.status()}): `{argv_str(cfg['build'])}`\n{tail(out)}")

    results: list[tuple[str | None, str, RunResult, RunResult]] = []
    noun = "fixtures" if kind == "full" else "module runs"
    out_of_budget = False
    if build_ok:
        for module, extra_args, unit_fixtures in units:
            if out_of_budget:
                break
            for rel in unit_fixtures:
                if deadline.expired():
                    problems.append(
                        f"parity run exceeded the {int(BUDGET_S)} s budget after {len(results)} of {planned} "
                        f"{noun}; make the drivers faster or raise the hook timeout and BUDGET_S together"
                    )
                    out_of_budget = True
                    break
                stdin_bytes = (root / rel).read_bytes() if cfg["input_mode"] == "stdin" else None
                t = min(cfg["per_run_timeout_s"], deadline.remaining())
                c_res = run(substitute(cfg["c_cmd"] + extra_args, rel), root, t, stdin_bytes)
                t = min(cfg["per_run_timeout_s"], deadline.remaining())
                r_res = run(substitute(cfg["rust_cmd"] + extra_args, rel), root, t, stdin_bytes)
                for res in (c_res, r_res):
                    if res.start_error and res.start_error not in problems:
                        problems.append(res.start_error)
                results.append((module, rel, c_res, r_res))

    exc_path = root / cfg["exceptions_file"]
    rows: list[dict[str, Any]] = []
    if exc_path.is_file():
        rows, parse_problems = parse_exceptions(exc_path.read_text(encoding="utf-8", errors="replace"))
        problems += parse_problems
    if units:
        passed_ids, test_problems = check_exception_tests(root, cfg, rows, deadline, method)
        problems += test_problems
    else:
        passed_ids = set()
        method["exceptions_test"] = "not run (nothing compared)"

    rows_by_fixture: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        for f in row["fixtures"]:
            rows_by_fixture.setdefault(f, []).append(row)
            if f not in fixture_now:
                problems.append(f"exception {row['id']} names fixture `{f}`, which is not matched by `fixtures`")

    fixture_rows: list[dict[str, Any]] = []
    divergence_details: list[str] = []
    exceptions_used: list[dict[str, str]] = []
    n_identical = n_excepted = n_diverged = 0
    for module, rel, c_res, r_res in results:
        same = identical(c_res, r_res, cfg["compare_stderr"])
        row_list = rows_by_fixture.get(rel, [])
        if same:
            n_identical += 1
            result = "identical"
            if kind != "full":
                if row_list:
                    result = "identical (" + ", ".join(r["id"] for r in row_list) + " not needed for this module)"
            else:
                for row in row_list:
                    problems.append(f"stale exception {row['id']}: fixture `{rel}` now matches C exactly; remove or fix the row")
                    result = f"identical (stale {row['id']})"
        else:
            ok_rows = [row for row in row_list if row["id"] in passed_ids]
            if ok_rows:
                n_excepted += 1
                result = "exception " + ", ".join(r["id"] for r in ok_rows)
                for row in ok_rows:
                    exceptions_used.append({"id": row["id"], "fixture": rel, "test": row["test"]})
            else:
                n_diverged += 1
                result = "DIVERGED"
                detail = describe_divergence(rel, c_res, r_res, cfg["compare_stderr"], module)
                if row_list:
                    detail += "\n  (named by " + ", ".join(r["id"] for r in row_list) + ", but its test is missing or failing)"
                divergence_details.append(detail)
        fixture_row: dict[str, Any] = {} if kind == "full" else {"module": module}
        fixture_row.update({
            "fixture": rel,
            "result": result,
            "c_status": c_res.status(),
            "rust_status": r_res.status(),
            "c_stdout_sha256": sha256_bytes(c_res.stdout),
            "rust_stdout_sha256": sha256_bytes(r_res.stdout),
        })
        fixture_rows.append(fixture_row)

    if problems or n_diverged:
        verdict = "FAIL"
    elif results:
        verdict = "PASS"
    elif kind == "modules" and not units:
        verdict = "SKIPPED"
    else:
        verdict = "FAIL"
    counts_text = (
        f"{len(results)} compared: {n_identical} identical, "
        f"{n_excepted} logged exception{'s' if n_excepted != 1 else ''}, {n_diverged} diverged; "
        f"{len(problems)} gate problem{'s' if len(problems) != 1 else ''}."
    )
    if kind == "full":
        summary = f"{len(fixtures)} fixtures, " + counts_text
    elif units:
        summary = (
            f"{len(units)} module{'s' if len(units) != 1 else ''} run ({', '.join(run_names)}): "
            f"{planned} module runs, " + counts_text
        )
    else:
        summary = (
            f"No module is ready ({len(modules or {})} defined); nothing compared; "
            f"{len(problems)} gate problem{'s' if len(problems) != 1 else ''}."
        )
    if kind == "partial":
        summary = f"{len(not_ready)} of {len(modules or {})} modules not ready, so the full compare did not run. " + summary
    not_covered = ["Inputs outside the fixture globs; the gate proves parity only on the listed fixtures."]
    if not cfg["compare_stderr"]:
        not_covered.append("stderr output (set `compare_stderr` to include it).")
    not_covered.append(
        "Independence of the Rust driver: the gate only checks that it is not the same executable as the C driver."
    )
    if kind != "full":
        not_covered.append("The full compare: it runs only on the final stop, once every module is ready.")
        if not_run:
            not_covered.append("Modules not run in this mode: " + ", ".join(f"`{n}`" for n in not_run) + ".")
        not_covered.append("Stale exception rows: only the full compare flags them.")
    report: dict[str, Any] = {
        "result": verdict,
        "summary": summary,
        "tree_hash": tree_hash,
        "run_key": cache_key,
        "method": method,
        "reproduce": reproduce_cmd(root, flags),
        "counts": {
            "fixtures": len(fixtures),
            "compared": len(results),
            "identical": n_identical,
            "exceptions": n_excepted,
            "diverged": n_diverged,
            "problems": len(problems),
        },
        "fixtures": fixture_rows,
        "divergence_details": divergence_details,
        "exceptions_used": exceptions_used,
        "problems": problems,
        "pins": pins_notes,
        "not_covered": not_covered,
    }
    if modules is not None:
        report["mode"] = kind
        report["modules_run"] = [] if kind == "full" else run_names
        report["modules_not_run"] = not_run
        if kind != "full":
            report["counts"]["module_runs"] = planned
    write_reports(root, report, md_path, json_path)

    followup: str | None = None
    if verdict == "FAIL":
        if kind == "modules":
            names = ", ".join(f"`{n}`" for n in run_names) or "(none ready)"
            parts = [f"Parity gate (C-to-Rust) failed for module(s) {names} in `{root}`: {summary}"]
        else:
            parts = [f"Parity gate (C-to-Rust) failed for `{root}`: {summary}"]
        if problems:
            parts.append("Gate problems:\n" + "\n".join(f"- {p}" for p in problems[:MAX_LISTED * 2]))
        if divergence_details:
            shown = divergence_details[:MAX_LISTED]
            parts.append(f"Divergences (showing {len(shown)} of {len(divergence_details)}):\n" + "\n".join(shown))
        parts.append(f"Full report: `{md_path}`. Re-run by hand: {reproduce_cmd(root, flags)}")
        tail_note = " Do not edit or delete fixtures, loosen `.cursor/parity.json`, or touch the gate's state."
        if modules is not None:
            tail_note += " A module marked ready stays ready, with the same `driver_args` and `fixtures`."
        parts.append(FINAL_INSTRUCTION + tail_note)
        followup = "\n\n".join(parts)
        if len(followup) > MAX_FOLLOWUP_CHARS:
            followup = followup[:MAX_FOLLOWUP_CHARS] + f"\n... (truncated; see {md_path})\n\n" + FINAL_INSTRUCTION

    state["oracle_pins"] = oracle_pins
    state["fixture_pins"] = fixture_pins
    if module_pins or "module_pins" in state:
        state["module_pins"] = module_pins
    entry = {"tree_hash": tree_hash, "verdict": verdict, "followup": followup or ""}
    if kind == "full":
        state["last"] = entry
    else:
        last_modules[cache_key] = entry
        state["last_modules"] = last_modules
    save_state(root, state)
    log(f"{root}: {verdict}: {summary}")
    return followup


def augment_path() -> None:
    parts = os.environ.get("PATH", "").split(os.pathsep)
    extra = [str(Path.home() / ".cargo" / "bin"), "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
    for d in extra:
        if d not in parts and Path(d).is_dir():
            parts.append(d)
    os.environ["PATH"] = os.pathsep.join(p for p in parts if p)


def emit(obj: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(obj))
    sys.stdout.flush()


def parse_args(args: list[str]) -> tuple[dict[str, Any], str | None]:
    """Parse `--force`, `--event NAME`, `--module NAME` (repeatable); `--flag=value` also works."""
    opts: dict[str, Any] = {"force": False, "event": None, "modules": []}
    i = 0
    while i < len(args):
        a = args[i]
        i += 1
        if a == "--force":
            opts["force"] = True
            continue
        key, sep, value = a.partition("=")
        if key not in ("--event", "--module"):
            return opts, f"unknown argument {a!r}"
        if not sep:
            if i >= len(args):
                return opts, f"{key} needs a value"
            value = args[i]
            i += 1
        if key == "--event":
            if value not in EVENTS:
                return opts, f"--event must be one of {', '.join(EVENTS)}, not {value!r}"
            opts["event"] = value
        else:
            if not MODULE_NAME_RE.fullmatch(value):
                return opts, f"--module {value!r} is not a valid module name"
            opts["modules"].append(value)
    return opts, None


def find_repo_root(start: Path) -> Path:
    for d in (start, *start.parents):
        if (d / CONFIG_REL).is_file() or (d / ".git").exists():
            return d
    return start


def payload_roots(payload: dict[str, Any]) -> list[Path]:
    """Workspace roots from the payload, else its `cwd`, else CURSOR_PROJECT_DIR, else this process's cwd."""
    roots = payload.get("workspace_roots")
    listed = [Path(r) for r in roots if isinstance(r, str) and r] if isinstance(roots, list) else []
    if listed:
        return listed
    cwd = payload.get("cwd")
    if isinstance(cwd, str) and cwd:
        log(f"no workspace_roots in hook input; using its cwd {cwd}")
        return [find_repo_root(Path(cwd))]
    project = os.environ.get("CURSOR_PROJECT_DIR")
    if project:
        log(f"no workspace_roots or cwd in hook input; using CURSOR_PROJECT_DIR {project}")
        return [Path(project)]
    here = Path.cwd()
    log(f"no workspace_roots, cwd, or CURSOR_PROJECT_DIR; using the hook's own cwd {here}")
    return [find_repo_root(here)]


def looks_like_port(root: Path) -> bool:
    return (root / CONFIG_REL).is_file() or (root / "Cargo.toml").is_file()


def main(argv: list[str]) -> int:
    opts, arg_error = parse_args(argv[1:])
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw) if raw.strip() else {}
    except json.JSONDecodeError as e:
        log(f"hook input is not JSON ({e}); allowing stop")
        emit({})
        return 0
    if not isinstance(payload, dict):
        log("hook input is not a JSON object; allowing stop")
        emit({})
        return 0
    payload_event = payload.get("hook_event_name")
    event = opts["event"] or (payload_event if payload_event in EVENTS else "stop")
    if opts["event"] and payload_event in EVENTS and payload_event != opts["event"]:
        log(f"--event {opts['event']} but hook input says {payload_event}; using --event")
    status = payload.get("status")
    if event == "stop" and status != "completed":
        emit({})
        return 0
    if event == "subagentStop" and status is not None and status != "completed":
        log(f"subagent status is {status!r}; not gating")
        emit({})
        return 0
    augment_path()

    messages: list[str] = []
    for entry in payload_roots(payload):
        root = entry.resolve()
        if not root.is_dir():
            continue
        if arg_error:
            log(f"invalid arguments: {arg_error}")
            if looks_like_port(root):
                messages.append(
                    f"Parity gate: invalid arguments in the hook command ({arg_error}), so parity is unproven for "
                    f"`{root}`. Stop and tell the user to fix .cursor/hooks.json."
                )
            continue
        try:
            msg = gate_root(root, opts["force"], event, opts["modules"])
        except Exception:  # noqa: BLE001 -- fail closed: any internal error in a port repo keeps the agent working
            tb = tail(traceback.format_exc(), 3000)
            log(f"{root}: internal error\n{tb}")
            msg = (
                f"Parity gate crashed while checking `{root}`, so parity is unproven. Fix the cause if it is in "
                f"this repo (config, drivers, fixtures); otherwise stop and tell the user.\n{tb}"
                if looks_like_port(root) else None
            )
        if msg:
            messages.append(msg)

    emit({"followup_message": "\n\n---\n\n".join(messages)} if messages else {})
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
