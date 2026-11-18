#!/usr/bin/env python3
"""Agreement between the TypeSafe judge and the Sonnet judges' verdicts on the frozen held-out set.

usage: calibrate.py [--limit N] [--workers N]
Reads ~/.local/share/huh-dev/heldout/{judged-*.md, <answers dirs>/answers.json, errors.json, env/}.
"""
import argparse, collections, concurrent.futures as cf, json, os, pathlib, sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import judge  # noqa: E402

H = pathlib.Path.home() / ".local/share/huh-dev/heldout"
USERS = {
    "beginner": "A Linux beginner on an Arch Linux desktop with the Omarchy setup (Hyprland), working in their home folder.",
    "sysadmin": "An experienced sysadmin or developer, working in a project folder.",
}
FOLDERS = {"beginner": "env/home", "sysadmin": "env/proj"}
# (judged table, answers before, answers after)
TABLES = [("judged-1bfd64d.md", "baseline", "after-1bfd64d"), ("judged-69904d8.md", "after-fix4", "after-69904d8")]


def folder_listing(part):
    root = H / FOLDERS[part]
    return sorted(p.name + ("/" if p.is_dir() else "") for p in root.iterdir() if not p.name.startswith("."))


def labelled():
    """Unique (part, key, answer text) -> human label; conflicting labels for the same text are dropped."""
    labels, conflicts = {}, 0
    for table, before, after in TABLES:
        answers = {d: json.loads((H / d / "answers.json").read_text()) for d in (before, after)}
        for line in (H / table).read_text().splitlines():
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if len(cells) < 4 or cells[0] not in ("beginner", "sysadmin", "errors"):
                continue
            part, key = cells[0], cells[1]
            for which, label in ((before, cells[2]), (after, cells[3])):
                label = label.split()[0].lower() if label else ""
                if label not in ("good", "bad"):
                    continue
                text = answers[which][part].get(key)
                if text is None:
                    continue
                k = (part, key, text)
                if k in labels and labels[k] != label:
                    conflicts += 1
                    labels[k] = None
                elif k not in labels:
                    labels[k] = label
    return {k: v for k, v in labels.items() if v}, conflicts


def run_one(item, errors):
    (part, key, text), label = item
    if part == "errors":
        case = errors[key]
        r = judge.judge_error(case.get("cmd", ""), case["output"], text)
    else:
        r = judge.judge_request(key, text, USERS[part], folder_listing(part))
    return part, key, text, label, r


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", default=str(H / "calibration.json"))
    args = ap.parse_args()
    items, conflicts = labelled()
    items = sorted(items.items())
    if args.limit:
        items = items[: args.limit]
    errors = {c["id"]: c for c in json.loads((H / "errors.json").read_text())}
    print(f"{len(items)} labelled answers ({conflicts} dropped: the two human judges disagreed on the same text)")
    results = []
    with cf.ThreadPoolExecutor(args.workers) as pool:
        for res in pool.map(lambda it: run_one(it, errors), items):
            results.append(res)
    table = collections.Counter()
    rows = []
    for part, key, text, label, r in results:
        table[(part, label, r["verdict"])] += 1
        rows.append({"part": part, "key": key, "human": label, "typesafe": r["verdict"], "why": r["why"],
                     "answers": r["answers"], "answer_text": text})
    pathlib.Path(args.out).write_text(json.dumps(rows, indent=1, ensure_ascii=False))
    for part in ("beginner", "sysadmin", "errors"):
        sub = [x for x in rows if x["part"] == part]
        if not sub:
            continue
        decided = [x for x in sub if x["typesafe"] != "unsure"]
        agree = sum(x["human"] == x["typesafe"] for x in decided)
        print(f"{part:9} n={len(sub):3}  decided={len(decided):3}  agree={agree}/{len(decided)} "
              f"({100 * agree / max(1, len(decided)):.0f}%)  unsure={len(sub) - len(decided)}  "
              f"human bad→ts good={sum(x['human']=='bad' and x['typesafe']=='good' for x in sub)}  "
              f"human good→ts bad={sum(x['human']=='good' and x['typesafe']=='bad' for x in sub)}")
    print(f"spent so far: ${judge.spent_dollars():.4f}")


if __name__ == "__main__":
    main()
