import json, os, re, subprocess, sys, time, glob, tomllib
from concurrent.futures import ThreadPoolExecutor
BIN = "/Users/andy/tmp/modernize/target/debug/cargo-modernize"
fetch = json.load(open("data/fetch.json"))
KEEP = int(sys.argv[1]) if len(sys.argv) > 1 else 30
# first KEEP viable per year, in the shuffled candidate order
chosen, count = [], {}
for r in fetch:
    if r["status"] == "ok" and count.get(r["year"], 0) < KEEP:
        chosen.append(r); count[r["year"]] = count.get(r["year"], 0) + 1
# ground-truth regexes: old idiom -> rule ids that should have fired
GT = {
  "try!":           (r"\btry!\s*\(", ["std/try-macro"]),
  "lazy_static!":   (r"\blazy_static!\s*[{(]", ["std/lazy_static"]),
  "mem::uninit":    (r"mem::uninitialized\s*\(", ["std/mem-uninitialized"]),
  "trim_left/right":(r"\.trim_(left|right)(_matches)?\s*\(", ["rusty/trim_left", "rusty/trim_right"]),
  "max_value()":    (r"\b[iu](8|16|32|64|128|size)::(max|min)_value\s*\(\)", ["rusty/max-value-fn"]),
  "std::u32::MAX":  (r"\b(std|core)::([iu](8|16|32|64|128|size)|f32|f64)::(MAX|MIN|EPSILON|INFINITY|NAN|MIN_POSITIVE|NEG_INFINITY)\b", ["std/legacy-numeric-consts"]),
  "extern crate":   (r"^\s*(#\[macro_use\]\s*)?extern crate\s+(?!std\b|core\b|alloc\b|test\b|proc_macro\b)\w+", ["edition/extern-crate"]),
  "#[macro_use]":   (r"#\[macro_use\]", ["edition/macro-use"]),
  "ATOMIC_*_INIT":  (r"\bATOMIC_(USIZE|ISIZE|BOOL|U\d+|I\d+)_INIT\b", ["std/atomic-init-consts"]),
  "fn description": (r"fn description\s*\(&self\)", ["std/error-description"]),
  "compare_and_swap":(r"\.compare_and_swap\s*\(", ["walk/compare-and-swap"]),
  "== None":        (r"[!=]=\s*None\b", ["walk/none-compare"]),
  ".len() == 0":    (r"\.len\(\)\s*(==\s*0\b|!=\s*0\b|>\s*0\b)", ["walk/len-zero"]),
  "sleep_ms":       (r"thread::sleep_ms\s*\(", ["walk/sleep-ms"]),
  "error_chain!":   (r"\berror_chain!\s*\{", ["deprecated/error-chain-macro"]),
  "Box<Fn(..)>":    (r"<\s*(Fn|FnMut|FnOnce)\s*\(", []),   # syn 3 can't parse: file is skipped
}
GT_RE = {k: (re.compile(p, re.M), ids) for k, (p, ids) in GT.items()}
def strip_comments(t):
    t = re.sub(r"/\*.*?\*/", "", t, flags=re.S)
    return re.sub(r"//[^\n]*", "", t)
def rs_files(d):
    out = []
    for root, dirs, files in os.walk(d):
        dirs[:] = [x for x in dirs if x not in ("target", ".git", "node_modules")]
        out += [os.path.join(root, f) for f in files if f.endswith(".rs")]
    return out
def run(r):
    d = r["dir"]; res = {k: r[k] for k in ("year", "repo", "top", "sub", "commit", "commit_date")}
    files = rs_files(d); texts = {}
    for f in files:
        try: texts[f] = strip_comments(open(f, errors="replace").read())
        except Exception: pass
    res["rs_files"] = len(files); res["loc"] = sum(t.count("\n") for t in texts.values())
    man = tomllib.loads(open(d + "/Cargo.toml", errors="replace").read()) if True else {}
    try: man = tomllib.load(open(d + "/Cargo.toml", "rb"))
    except Exception as e: man = {}; res["manifest_parse_error"] = str(e)[:100]
    pkg = man.get("package", {})
    res["edition"] = str(pkg.get("edition", "2015")); res["workspace"] = "workspace" in man
    res["deps"] = sorted(set(list(man.get("dependencies", {})) + list(man.get("dev-dependencies", {}))))
    gt = {}
    for k, (rx, ids) in GT_RE.items():
        n = sum(len(rx.findall(t)) for t in texts.values())
        if n: gt[k] = n
    res["groundtruth"] = gt
    t0 = time.time()
    try:
        p = subprocess.run([BIN, "modernize", "--manifest-path", d + "/Cargo.toml", "--format", "json", "--fail-on", "off"],
                           capture_output=True, text=True, timeout=420)
        res["exit"] = p.returncode; res["secs"] = round(time.time() - t0, 1)
        err = p.stderr
        res["skipped_files"] = re.findall(r"warning: skipping (\S+): (.*)", err)
        if p.returncode != 0 or not p.stdout.strip():
            res["failure"] = [l for l in err.splitlines() if l.startswith("error")][:3] or err.strip().splitlines()[-3:]
            res["findings"] = []
        else:
            v = json.loads(p.stdout); fs = []
            res["pkg_editions"] = [pk["edition"] for pk in v["packages"]]; res["pkg_rust_versions"] = [pk["rust_version"] for pk in v["packages"]]
            for pk in v["packages"]:
                for f in pk["findings"]:
                    fs.append({"rule": f["rule"], "level": f["level"], "category": f.get("category"), "plan": f.get("plan"), "file": f["file"], "line": f.get("line"), "fix": bool(f.get("fix"))})
            res["findings"] = fs
    except subprocess.TimeoutExpired:
        res["failure"] = ["timeout"]; res["findings"] = []
    return res
with ThreadPoolExecutor(4) as ex:
    results = list(ex.map(run, chosen))
json.dump(results, open("data/results.json", "w"), indent=1)
print(len(results), "runs;", sum(1 for r in results if r.get("failure")), "failed")
