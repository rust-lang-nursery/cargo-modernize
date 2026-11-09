import json, re, os, collections, sys
sys.path.insert(0, "tools")
R = json.load(open("data/results.json"))
fetch = {(f["year"], f["repo"]): f for f in json.load(open("data/fetch.json"))}
exec(open("tools/run.py").read().split("GT_RE =")[0].split("GT = ")[1].join(["GT = ", ""]) if False else "")
src = open("tools/run.py").read()
gt_src = src[src.index("GT = {"):src.index("GT_RE =")]
ns = {}; exec(gt_src, ns); GT = ns["GT"]
GT_RULES = {"try!": ["std/try-macro"], "lazy_static!": ["std/lazy_static"], "mem::uninit": ["std/mem-uninitialized"],
 "trim_left/right": ["rusty/trim_left", "rusty/trim_right"], "max_value()": ["rusty/max-value-fn"], "std::u32::MAX": ["std/legacy-numeric-consts"],
 "extern crate": ["edition/extern-crate"], "#[macro_use]": ["edition/macro-use"], "ATOMIC_*_INIT": ["std/atomic-init-consts"],
 "fn description": ["std/error-description"], "compare_and_swap": ["walk/compare-and-swap"], "== None": ["walk/none-compare"],
 ".len() == 0": ["walk/len-zero"], "sleep_ms": ["walk/sleep-ms"], "error_chain!": ["deprecated/error-chain-macro"]}
def strip_comments(t):
    t = re.sub(r"/\*.*?\*/", lambda m: "\n" * m.group(0).count("\n"), t, flags=re.S)
    return re.sub(r"//[^\n]*", "", t)
def in_macro_rules(text, pos):
    # inside a `macro_rules! name { ... }` body? brace matching from each macro_rules! start
    for m in re.finditer(r"macro_rules!\s*\w+\s*\{", text):
        depth, i = 1, m.end()
        while i < len(text) and depth:
            depth += (text[i] == "{") - (text[i] == "}"); i += 1
        if m.end() <= pos < i: return True
    return False
import subprocess, tomllib
def manifests(d):
    out = {}
    for root, dirs, files in os.walk(d):
        dirs[:] = [x for x in dirs if x not in ("target", ".git", "node_modules") and not x.startswith(".")]
        if "Cargo.toml" in files:
            try: m = tomllib.load(open(root + "/Cargo.toml", "rb"))
            except Exception: m = {}
            ed = m.get("package", {}).get("edition", "2015"); out[root] = str(ed) if not isinstance(ed, dict) else "workspace"
    return out
causes = collections.defaultdict(collections.Counter); examples = collections.defaultdict(list)
for r in R:
    if r.get("failure"): continue
    d = fetch[(r["year"], r["repo"])]["dir"]
    skipped = {f for f, _ in r["skipped_files"]}
    dabs = os.path.abspath(d)
    pm = subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", dabs + "/Cargo.toml"], capture_output=True, text=True)
    members = {os.path.dirname(x["manifest_path"]) for x in json.loads(pm.stdout)["packages"]}
    mans = manifests(dabs)
    found_files = {}
    for f in r["findings"]:
        found_files.setdefault(f["rule"], set()).add(f["file"])
    for idiom, ids in GT_RULES.items():
        rx = re.compile(GT[idiom][0], re.M)
        for root, dirs, files in os.walk(d):
            dirs[:] = [x for x in dirs if x not in ("target", ".git")]
            for fn in files:
                if not fn.endswith(".rs"): continue
                path = os.path.join(root, fn); rel = os.path.relpath(path, d)
                text = strip_comments(open(path, errors="replace").read())
                ms = list(rx.finditer(text))
                if not ms: continue
                hit_here = any(rel in found_files.get(i, ()) or any(rel.endswith(x) or x.endswith(rel) for x in found_files.get(i, ())) for i in ids)
                if hit_here: continue
                owner = max((m for m in mans if os.path.abspath(path).startswith(m + "/")), key=len, default=None)
                ed = mans.get(owner, "2015")
                if owner not in members: c = "crate not analyzed (not a workspace member)"
                elif path in skipped or any(path.endswith(s.split(d)[-1]) for s in skipped): c = "file skipped (syn parse error)"
                elif idiom in ("extern crate", "#[macro_use]") and ed == "2015": c = "edition 2015 (by design)"
                elif all(in_macro_rules(text, m.start()) for m in ms): c = "inside macro_rules! body"
                else: c = "unexplained"
                causes[idiom][c] += len(ms)
                if c == "unexplained" and len(examples[idiom]) < 4:
                    line = text[:ms[0].start()].count("\n") + 1
                    examples[idiom].append((r["year"], r["repo"], f"{rel}:{line}", text.splitlines()[line - 1].strip()[:90]))
for idiom in GT_RULES:
    print(f"{idiom:18}", dict(causes[idiom]))
    for e in examples[idiom]: print("      ", e)
json.dump({k: dict(v) for k, v in causes.items()}, open("data/miss_causes.json", "w"), indent=1)
json.dump(examples, open("data/miss_examples.json", "w"), indent=1)
