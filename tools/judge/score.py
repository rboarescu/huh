#!/usr/bin/env python3
"""Score every answer with the TypeSafe judge.

  score.py frozen <answers.json> [--out report.json]
      the frozen held-out set (~/.local/share/huh-dev/heldout), all parts but explain
  score.py replay <huh> <requests.txt> <folder> beginner|sysadmin [--out report.json]
      run huh (which runs nothing itself) on each request inside <folder>, then judge it

Prints good / bad / unsure rates per part and the main reasons for "bad".
"""
import argparse, collections, concurrent.futures as cf, json, pathlib, subprocess, sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import judge  # noqa: E402
from calibrate import H, USERS, folder_listing  # noqa: E402


def listing(folder: pathlib.Path):
    return sorted(p.name + ("/" if p.is_dir() else "") for p in folder.iterdir() if not p.name.startswith("."))


def summarise(rows):
    for part in sorted({r["part"] for r in rows}):
        sub = [r for r in rows if r["part"] == part]
        c = collections.Counter(r["verdict"] for r in sub)
        decided = c["good"] + c["bad"]
        print(f"{part:9} n={len(sub):3}  good={c['good']} ({100 * c['good'] / max(1, decided):.0f}% of decided)  "
              f"bad={c['bad']}  unsure={c['unsure']}")
        reasons = collections.Counter(r["why"] for r in sub if r["verdict"] == "bad")
        for why, n in reasons.most_common(6):
            print(f"            {n:3}  {why}")
    print(f"spent so far: ${judge.spent_dollars():.4f}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["frozen", "replay"])
    ap.add_argument("args", nargs="+")
    ap.add_argument("--out")
    ap.add_argument("--workers", type=int, default=6)
    a = ap.parse_args()
    jobs = []
    if a.mode == "frozen":
        answers = json.loads(pathlib.Path(a.args[0]).read_text())
        errors = {c["id"]: c for c in json.loads((H / "errors.json").read_text())}
        for part in ("beginner", "sysadmin"):
            folder = folder_listing(part)
            for req, text in answers[part].items():
                jobs.append((part, req, text, lambda req=req, text=text, part=part, folder=folder:
                             judge.judge_request(req, text, USERS[part], folder)))
        for cid, text in answers["errors"].items():
            c = errors[cid]
            jobs.append(("errors", cid, text, lambda c=c, text=text: judge.judge_error(c.get("cmd", ""), c["output"], text)))
    else:
        huh, reqfile, folder, part = a.args
        folder = pathlib.Path(folder)
        names = listing(folder)
        for req in (r.strip() for r in pathlib.Path(reqfile).read_text().splitlines()):
            if not req or req.startswith("#"):
                continue
            p = subprocess.run([huh, "--no-ask", req], cwd=folder, capture_output=True, text=True, timeout=20)
            text = (p.stdout + p.stderr).strip()
            jobs.append((part, req, text, lambda req=req, text=text: judge.judge_request(req, text, USERS[part], names)))
    rows = []
    with cf.ThreadPoolExecutor(a.workers) as pool:
        futures = {pool.submit(fn): (part, key, text) for part, key, text, fn in jobs}
        for fut in cf.as_completed(futures):
            part, key, text = futures[fut]
            r = fut.result()
            rows.append({"part": part, "key": key, "verdict": r["verdict"], "why": r["why"],
                         "answers": r["answers"], "answer_text": text})
    rows.sort(key=lambda r: (r["part"], r["key"]))
    if a.out:
        pathlib.Path(a.out).write_text(json.dumps(rows, indent=1, ensure_ascii=False))
    summarise(rows)


if __name__ == "__main__":
    main()
