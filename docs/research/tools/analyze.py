import json, collections, re, os
R = json.load(open("data/results.json"))
YEARS = ["2015", "2018", "2021", "2024"]
GT_RULES = {
 "try!": ["std/try-macro"], "lazy_static!": ["std/lazy_static"], "mem::uninit": ["std/mem-uninitialized"],
 "trim_left/right": ["rusty/trim_left", "rusty/trim_right"], "max_value()": ["rusty/max-value-fn"],
 "std::u32::MAX": ["std/legacy-numeric-consts"], "extern crate": ["edition/extern-crate"], "#[macro_use]": ["edition/macro-use"],
 "ATOMIC_*_INIT": ["std/atomic-init-consts"], "fn description": ["std/error-description"], "compare_and_swap": ["walk/compare-and-swap"],
 "== None": ["walk/none-compare"], ".len() == 0": ["walk/len-zero"], "sleep_ms": ["walk/sleep-ms"], "error_chain!": ["deprecated/error-chain-macro"],
}
def skipped_loc(r):
    n = 0
    for f, _ in r["skipped_files"]:
        try: n += open(f, errors="replace").read().count("\n")
        except Exception: pass
    return n
out = {"years": {}}
print("== overview")
for y in YEARS:
    rs = [r for r in R if r["year"] == y]; ok = [r for r in rs if not r.get("failure")]
    fs = [f for r in ok for f in r["findings"]]
    # exclude whole-project edition nudge from "hit" (every old crate hits it)
    fs2 = [f for f in fs if f["rule"] != "edition/2024"]
    hit = [r for r in ok if any(f["rule"] != "edition/2024" for f in r["findings"])]
    loc = sum(r["loc"] for r in ok); skl = sum(skipped_loc(r) for r in ok)
    by_cat = collections.Counter(f["category"] for f in fs2); by_lvl = collections.Counter(f["level"] for f in fs2)
    fixable = sum(1 for f in fs2 if f["fix"])
    ed = collections.Counter(e for r in ok for e in set(r.get("pkg_editions", [])))
    rep = {"repos": len(rs), "ok": len(ok), "failed": len(rs) - len(ok), "repos_with_hit": len(hit), "findings": len(fs2), "kloc": round(loc / 1000, 1),
           "per_kloc": round(len(fs2) / (loc / 1000), 2) if loc else 0, "files_skipped_repos": sum(1 for r in ok if r["skipped_files"]),
           "skipped_loc": skl, "skipped_pct": round(100 * skl / loc, 1) if loc else 0, "fixable": fixable, "by_category": dict(by_cat), "by_level": dict(by_lvl), "editions": dict(ed)}
    out["years"][y] = rep
    print(y, json.dumps(rep))
# rule table by year
rule_year = collections.defaultdict(lambda: collections.Counter()); rule_repos = collections.defaultdict(lambda: collections.Counter())
for r in R:
    if r.get("failure"): continue
    seen = set()
    for f in r["findings"]:
        rule_year[f["rule"]][r["year"]] += 1
        seen.add(f["rule"])
    for s in seen: rule_repos[s][r["year"]] += 1
print("\n== top rules (findings / repos) by year")
rows = sorted(rule_year, key=lambda k: -sum(rule_year[k].values()))
for k in rows[:45]:
    print(f"{k:38}", "  ".join(f"{y}:{rule_year[k][y]:4}/{rule_repos[k][y]:2}" for y in YEARS))
out["rules"] = {k: {y: [rule_year[k][y], rule_repos[k][y]] for y in YEARS} for k in rows}
# parse failures
print("\n== skipped files (syn parse errors)")
err = collections.Counter(); ex = {}
for r in R:
    for f, e in r["skipped_files"]:
        e = re.sub(r"`[^`]*`", "`…`", e); err[e] += 1; ex.setdefault(e, (r["year"], r["repo"], f.split("/")[-1]))
for e, n in err.most_common(10): print(n, e, ex[e])
out["parse_errors"] = err.most_common(15)
# metadata failures
print("\n== run failures")
for r in R:
    if r.get("failure"): print(r["year"], r["repo"], r["failure"][:2])
# ground truth recall
print("\n== ground-truth idiom recall (regex hit in source vs tool finding of expected rule)")
gt = collections.defaultdict(lambda: collections.defaultdict(lambda: [0, 0, 0, 0]))  # year -> idiom -> [src_hits, found, repos_src, repos_found]
for r in R:
    if r.get("failure"): continue
    for idiom, n in r["groundtruth"].items():
        if idiom not in GT_RULES: continue
        found = sum(1 for f in r["findings"] if f["rule"] in GT_RULES[idiom])
        g = gt[r["year"]][idiom]; g[0] += n; g[1] += found; g[2] += 1; g[3] += 1 if found else 0
for idiom in GT_RULES:
    cells = []
    for y in YEARS:
        g = gt[y].get(idiom)
        cells.append(f"{y}:{g[1]}/{g[0]} ({g[3]}/{g[2]}r)" if g else f"{y}:-")
    print(f"{idiom:18}", "  ".join(cells))
out["groundtruth"] = {y: {i: v for i, v in gt[y].items()} for y in YEARS}
json.dump(out, open("data/analysis.json", "w"), indent=1)
