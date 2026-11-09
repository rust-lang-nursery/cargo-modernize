import json, os, subprocess, collections
R = json.load(open("data/results.json")); F = {(f["year"], f["repo"]): f for f in json.load(open("data/fetch.json"))}
def manifests(d):
    out = []
    for root, dirs, files in os.walk(d):
        dirs[:] = [x for x in dirs if x not in ("target", ".git", "node_modules") and not x.startswith(".")]
        if "Cargo.toml" in files: out.append(root)
    return out
rows = []
for r in R:
    d = os.path.abspath(F[(r["year"], r["repo"])]["dir"])
    p = subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", d + "/Cargo.toml"], capture_output=True, text=True)
    if p.returncode: rows.append({**{k: r[k] for k in ("year", "repo")}, "meta": "failed"}); continue
    members = {os.path.dirname(x["manifest_path"]) for x in json.loads(p.stdout)["packages"]}
    mans = manifests(d)
    loc = collections.Counter()
    for root, dirs, files in os.walk(d):
        dirs[:] = [x for x in dirs if x not in ("target", ".git", "node_modules") and not x.startswith(".")]
        owner = max((m for m in mans if root == m or root.startswith(m + "/")), key=len, default=None)
        for f in files:
            if f.endswith(".rs"):
                try: n = open(os.path.join(root, f), errors="replace").read().count("\n")
                except Exception: n = 0
                loc["analyzed" if owner in members else "unanalyzed"] += n
    rows.append({"year": r["year"], "repo": r["repo"], "members": len(members), "manifests": len(mans), **loc})
json.dump(rows, open("data/coverage_loc.json", "w"), indent=1)
for y in ("2015", "2018", "2021", "2024"):
    rs = [x for x in rows if x["year"] == y and "analyzed" in x or x["year"] == y and "unanalyzed" in x]
    a = sum(x.get("analyzed", 0) for x in rs); u = sum(x.get("unanalyzed", 0) for x in rs)
    n = sum(1 for x in rs if x.get("unanalyzed", 0) > 0)
    print(y, "analyzed", a, "unanalyzed", u, f"{100*u/(a+u):.1f}%", "repos with unanalyzed crates:", n, "of", len(rs))
for x in sorted(rows, key=lambda x: -x.get("unanalyzed", 0))[:8]: print("  ", x["year"], x["repo"], x.get("unanalyzed"), "of", x.get("analyzed", 0) + x.get("unanalyzed", 0))
