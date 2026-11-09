import json, re, subprocess, sys
SRC = "src"
SNAP = {  # last commit before year end, from `git rev-list -1 --before`
}
def git(repo, *a):
    return subprocess.run(["git", "-C", f"{SRC}/{repo}", *a], capture_output=True, text=True).stdout
def snap(repo, year):
    return git(repo, "rev-list", "-1", f"--before={year}-12-31T23:59", "HEAD").strip()
out = {}
for year in (2015, 2018, 2021, 2024):
    rec = {"year": year, "snapshots": {}}
    # awesome-rust
    c = snap("awesome-rust", year); rec["snapshots"]["awesome-rust"] = c
    text = git("awesome-rust", "show", f"{c}:README.md")
    top = None; sub = None; items = []
    for line in text.splitlines():
        m = re.match(r"^(#{2,4})\s+(.*)", line)
        if m:
            name = re.sub(r"\[|\]\(.*?\)|\)", "", m.group(2)).strip()
            if len(m.group(1)) == 2: top, sub = name, None
            else: sub = name
        m = re.match(r"^\s*[*-]\s+\[([^\]]+)\]\((https://github\.com/([^/\s)]+)/([^/\s)#]+?))/?\)", line)
        if m and top:
            items.append({"top": top, "sub": sub, "label": m.group(1), "repo": f"{m.group(3)}/{m.group(4)}"})
    rec["awesome"] = items
    # cookbook
    c = snap("rust-cookbook", year)
    if c:
        rec["snapshots"]["rust-cookbook"] = c
        toml = git("rust-cookbook", "show", f"{c}:Cargo.toml")
        deps, sect = [], None
        for line in toml.splitlines():
            if line.startswith("["): sect = line.strip(); continue
            m = re.match(r"^([A-Za-z0-9_\-]+)\s*=", line)
            if m and sect and "dependencies" in sect: deps.append(m.group(1))
        rec["cookbook"] = sorted(set(deps))
    # blessed
    if year >= 2022:
        c = snap("blessed-rs", year)
        if c:
            rec["snapshots"]["blessed-rs"] = c
            d = json.loads(git("blessed-rs", "show", f"{c}:data/crates.json"))
            names = []
            def walk(g):
                for p in g.get("purposes", []):
                    for r in p.get("recommendations", []): names.append(r["name"])
                for s in g.get("subgroups", []): walk(s)
            for g in d["crate_groups"]: walk(g)
            rec["blessed"] = sorted(set(names))
    out[year] = rec
json.dump(out, open("data/recs.json", "w"), indent=1)
for y, r in out.items():
    from collections import Counter
    print(y, {k: (len(v) if isinstance(v, list) else v) for k, v in r.items() if k != "snapshots"}, Counter(i["top"] for i in r["awesome"]).most_common(4))
